// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

use crate::hive::node::Name;
use itertools::Itertools;
use nix_compat::log::{ActivityType, Field, ResultType};
use owo_colors::{FgColorDisplay, OwoColorize};
use std::{
    collections::VecDeque,
    fmt::Write,
    sync::OnceLock,
    time::{Duration, Instant},
};
use strip_ansi_escapes::strip_str;
use termion::{clear, cursor};
use tokio::sync::{
    mpsc::{self, UnboundedReceiver},
    oneshot,
};

use std::collections::HashMap;

#[derive(Default)]
pub enum NodeStatus {
    #[default]
    Pending,
    Running(String),
    Succeeded,
    Failed,
}

pub enum UiMessage {
    /// Initialise the status bar with many nodes at once.
    AddMany(Vec<Name>),
    SetStatus(Name, NodeStatus),
    /// Takeover the terminal, blocking new messages from being printed until
    /// `Release` is sent.
    ///
    /// Once the takeover request is completed, the oneshot channel will be
    /// consumed.
    Takeover(oneshot::Sender<()>),
    /// Indicate that the takeover is no longer necessary
    Release,
    /// Clear the status line, mostly for when the program is about to end
    Clear,
    /// Writes above the status line
    LogLine(Vec<u8>),
    /// Notes that a nix activity has begun
    ActivityBegin {
        node: Option<Name>,
        id: u64,
        activity_category: Option<ActivityCategory>,
    },
    /// Notes that a nix activity has ended
    ActivityEnd {
        node: Option<Name>,
        id: u64,
        result: Option<ResultType>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActivityCategory {
    Download,
    Upload,
    Build,
    BuildPending,
}

// reference: https://github.com/maralorn/nix-output-monitor/blob/8f8e7caf3c5a440683b12b99f93926966da312a5/lib/NOM/Builds.hs#L78
#[must_use]
pub fn field_is_localhost(field: &Field) -> bool {
    if let Field::String(to_string) = field
        && matches!(
            to_string.as_ref(),
            // deviates from nom, daemon appears in testing as a localhost
            b"" | b"local" | b"local://" | b"unix" | b"unix://" | b"daemon"
        )
    {
        return true;
    }

    false
}

pub fn create_activity_category(
    activity_type: &ActivityType,
    fields: Option<Vec<Field>>,
) -> Option<ActivityCategory> {
    if let Some(fields) = fields
        && matches!(activity_type, ActivityType::CopyPath)
    {
        // index 1 = "from", index 2 = "to"
        if fields.get(2).is_some_and(field_is_localhost) {
            return Some(ActivityCategory::Download);
        }

        if fields.get(1).is_some_and(field_is_localhost) {
            return Some(ActivityCategory::Upload);
        }

        return None;
    }

    match activity_type {
        ActivityType::FileTransfer | ActivityType::FetchTree => Some(ActivityCategory::Download),

        ActivityType::Build => Some(ActivityCategory::Build),

        ActivityType::BuildWaiting => Some(ActivityCategory::BuildPending),

        // Ignoring Substitute: makes downloads very noisy, this is raised for every cache hit not
        // just downloads themselves
        // Ignoring QueryPathInfo: again, this is on the order of kilobytes so its not worth tracking as
        // a big download
        // Ignoring CopyPath: Already covered in above if statement
        //
        // Other variants ignored for irrelevance
        _ => None,
    }
}

#[derive(Default, Debug)]
pub struct ActivityTracker {
    active_by_id: HashMap<u64, ActivityCategory>,

    pub active_downloads: usize,
    pub completed_downloads: usize,

    pub active_uploads: usize,
    pub completed_uploads: usize,

    pub active_builds: usize,
    pub pending_builds: usize,
    pub completed_builds: usize,
}

impl ActivityTracker {
    pub fn begin(&mut self, id: u64, category: Option<ActivityCategory>) {
        let Some(category) = category else {
            return;
        };

        self.active_by_id.insert(id, category);

        match category {
            ActivityCategory::Download => self.active_downloads += 1,
            ActivityCategory::Build => self.active_builds += 1,
            ActivityCategory::BuildPending => self.pending_builds += 1,
            ActivityCategory::Upload => self.active_uploads += 1,
        }
    }

    pub fn end(&mut self, id: u64) {
        if let Some(category) = self.active_by_id.remove(&id) {
            match category {
                ActivityCategory::Download => {
                    self.active_downloads = self.active_downloads.saturating_sub(1);
                    self.completed_downloads += 1;
                }
                ActivityCategory::Build => {
                    self.active_builds = self.active_builds.saturating_sub(1);
                    self.completed_builds += 1;
                }
                ActivityCategory::BuildPending => {
                    self.pending_builds = self.pending_builds.saturating_sub(1);
                }
                ActivityCategory::Upload => {
                    self.active_uploads = self.active_uploads.saturating_sub(1);
                    self.completed_uploads += 1;
                }
            }
        }
    }

    pub(crate) const fn get_counts(&self) -> [usize; 7] {
        [
            self.active_builds,
            self.completed_builds,
            self.pending_builds,
            self.active_downloads,
            self.completed_downloads,
            self.active_uploads,
            self.completed_uploads,
        ]
    }
}

pub struct Status {
    node_statuses: HashMap<Name, NodeStatus>,
    nix_activities: HashMap<Option<Name>, ActivityTracker>,

    began: Instant,
    show_progress: bool,
    previous_number_of_lines: usize,
}

pub static UI_SENDER: OnceLock<mpsc::UnboundedSender<UiMessage>> = OnceLock::new();
const MAX_NODE_NAME_LENGTH: usize = 20;

const ACTIVE_BUILDS_ICON: FgColorDisplay<owo_colors::colors::Yellow, str> =
    FgColorDisplay::new("⏵");
const COMPLETED_BUILDS_ICON: FgColorDisplay<owo_colors::colors::Green, str> =
    FgColorDisplay::new("✔");
const PENDING_BUILDS_ICON: FgColorDisplay<owo_colors::colors::Blue, str> = FgColorDisplay::new("⏸");
const ACTIVE_DOWNLOAD_ICON: FgColorDisplay<owo_colors::colors::Yellow, str> =
    FgColorDisplay::new("↓ ⏵");
const COMPLETED_DOWNLOAD_ICON: FgColorDisplay<owo_colors::colors::Green, str> =
    FgColorDisplay::new("↓ ✔");
const ACTIVE_UPLOAD_ICON: FgColorDisplay<owo_colors::colors::Yellow, str> =
    FgColorDisplay::new("↑ ⏵");
const COMPLETED_UPLOAD_ICON: FgColorDisplay<owo_colors::colors::Green, str> =
    FgColorDisplay::new("↑ ✔");

impl Status {
    fn new() -> Self {
        Self {
            node_statuses: HashMap::default(),
            nix_activities: HashMap::default(),
            began: Instant::now(),
            show_progress: false,
            previous_number_of_lines: 0,
        }
    }

    pub const fn show_progress(&mut self, show_progress: bool) {
        self.show_progress = show_progress;
    }

    #[must_use]
    fn num_finished(&self) -> usize {
        self.node_statuses
            .iter()
            .filter(|(_, status)| matches!(status, NodeStatus::Succeeded | NodeStatus::Failed))
            .count()
    }

    #[must_use]
    fn num_running(&self) -> usize {
        self.node_statuses
            .iter()
            .filter(|(_, status)| matches!(status, NodeStatus::Running(..)))
            .count()
    }

    #[must_use]
    fn num_failed(&self) -> usize {
        self.node_statuses
            .iter()
            .filter(|(_, status)| matches!(status, NodeStatus::Failed))
            .count()
    }

    #[must_use]
    pub fn get_msg(&self) -> String {
        if self.node_statuses.is_empty() {
            return String::new();
        }

        let mut msg = String::new();
        let _ = self.write_summary(&mut msg);

        let truncated_names = self.get_truncated_names();
        let max_name_len = truncated_names.values().map(String::len).max().unwrap_or(0);

        let node_lines = self.get_node_lines(&truncated_names, max_name_len);
        let max_line_len = node_lines
            .values()
            .map(|l| strip_str(l).len())
            .max()
            .unwrap_or(0);

        let (activities, widths) = self.get_activity_columns();

        for (name, line) in node_lines
            .into_iter()
            .sorted_by_key(|(n, _)| truncated_names.get(n).unwrap().clone())
        {
            let padding = " ".repeat(max_line_len.saturating_sub(strip_str(&line).len()));
            let activity = activities
                .get(&name)
                .map(|counts| Self::format_activity_row(counts, &widths))
                .unwrap_or_default();
            let _ = write!(&mut msg, "{line}{padding} {activity}");
        }

        msg
    }

    fn write_summary(&self, msg: &mut String) -> std::fmt::Result {
        write!(
            msg,
            "{}s Elapsed [Nodes {} / {}",
            self.began.elapsed().as_secs(),
            self.num_finished(),
            self.node_statuses.len()
        )?;

        let num_failed = self.num_failed();
        let num_running = self.num_running();

        match (num_failed, num_running) {
            (0, 0) => (),
            (f, 0) => write!(msg, " ({} Failed)", f.red())?,
            (0, r) => write!(msg, " ({} Deploying)", r.blue())?,
            (f, r) => write!(msg, " ({} Failed, {} Deploying)", f.red(), r.blue())?,
        }

        write!(msg, "]")
    }

    fn get_truncated_names(&self) -> HashMap<Name, String> {
        self.node_statuses
            .keys()
            .map(|name| {
                let truncated = if name.0.len() <= MAX_NODE_NAME_LENGTH {
                    name.0.to_string()
                } else {
                    format!(
                        "{}...",
                        name.0
                            .chars()
                            .take(MAX_NODE_NAME_LENGTH)
                            .collect::<String>()
                    )
                };

                (name.clone(), truncated)
            })
            .collect()
    }

    fn get_node_lines(
        &self,
        truncated_names: &HashMap<Name, String>,
        max_name_len: usize,
    ) -> HashMap<Name, String> {
        self.node_statuses
            .iter()
            .map(|(name, status)| {
                let truncated = truncated_names.get(name).unwrap();
                (
                    name.clone(),
                    format!(
                        "\n  {}{} {}",
                        truncated.bold(),
                        " ".repeat(max_name_len.saturating_sub(truncated.len())),
                        match status {
                            NodeStatus::Pending => "Waiting".dimmed().to_string(),
                            NodeStatus::Running(task) => format!("Running {}", task.blue())
                                .on_default_color()
                                .to_string(),
                            NodeStatus::Succeeded => "Finished".green().to_string(),
                            NodeStatus::Failed => "Failed".red().to_string(),
                        }
                    ),
                )
            })
            .collect()
    }

    // returns activity trackers, their cell values and the maximum width
    // for all the columns
    fn get_activity_columns(&self) -> (HashMap<Name, [String; 7]>, [usize; 7]) {
        let mut widths = [0; 7];
        let mut activities = HashMap::new();

        for (name, tracker) in &self.nix_activities {
            let Some(name) = name else { continue };

            let counts = tracker.get_counts().map(|c| c.to_string());
            for (i, count) in counts.iter().enumerate() {
                // replace this column max width with any that are higher
                widths[i] = widths[i].max(count.len());
            }

            activities.insert(name.clone(), counts);
        }

        (activities, widths)
    }

    fn format_activity_row(counts: &[String; 7], widths: &[usize; 7]) -> String {
        let pad = |i: usize, s: &String| {
            format!("{}{}", s, " ".repeat(widths[i].saturating_sub(s.len())))
        };

        format!(
            "{ACTIVE_BUILDS_ICON} {} ❘ {COMPLETED_BUILDS_ICON} {} ❘ {PENDING_BUILDS_ICON} {} ❘ {ACTIVE_DOWNLOAD_ICON} {} ❘ {COMPLETED_DOWNLOAD_ICON} {} | {ACTIVE_UPLOAD_ICON} {} | {COMPLETED_UPLOAD_ICON} {}",
            pad(0, &counts[0]).yellow(),
            pad(1, &counts[1]),
            pad(2, &counts[2]).blue(),
            pad(3, &counts[3]).yellow(),
            pad(4, &counts[4]).green(),
            pad(5, &counts[5]).yellow(),
            pad(6, &counts[6]).green(),
        )
    }

    pub fn clear<T: std::io::Write>(&self, writer: &mut T) {
        if !self.show_progress {
            return;
        }

        for _ in 0..self.previous_number_of_lines {
            let _ = write!(writer, "\r{}{}", clear::CurrentLine, cursor::Up(1));
        }

        let _ = write!(writer, "\r{}", clear::CurrentLine);
        let _ = writer.flush();
    }

    /// used when there is an interactive prompt
    pub fn wipe_out<T: std::io::Write>(&mut self, writer: &mut T) {
        if !self.show_progress {
            return;
        }

        self.clear(writer);
    }

    pub fn write_status<T: std::io::Write>(&mut self, writer: &mut T) {
        if !self.show_progress {
            return;
        }

        let msg = self.get_msg();

        let _ = write!(writer, "{msg}");
        let _ = writer.flush();

        // track how many lines we're about to draw
        self.previous_number_of_lines = msg.lines().count().saturating_sub(1);
    }
}

pub async fn status_tick_worker(mut rx: UnboundedReceiver<UiMessage>, show_progress: bool) {
    let mut status = Status::new();

    status.show_progress(show_progress);

    let mut ticker = tokio::time::interval(Duration::from_secs(1));
    let mut stderr = std::io::stderr();
    let mut log_queue: VecDeque<Vec<u8>> = VecDeque::with_capacity(100);

    // A single boolean represents the "taken over" state, where stdin is being
    // accepted from the user. A "depth" is not used as it is expected the
    // callers of `Takeover` respect the Semaphore.
    //
    // If there was ever multiple take overs at once (unlikely), this code would
    // need to be updated to track multiple takeovers at once.
    let mut taken_over = false;

    loop {
        tokio::select! {
            Some(msg) = rx.recv() => {
                match msg {
                    UiMessage::AddMany(names) => {
                        status.node_statuses.extend(
                            names
                                .iter()
                                .map(|name| (name.clone(), NodeStatus::Pending)),
                        );
                    },
                    UiMessage::SetStatus(name, value) => {
                        status.node_statuses.insert(name.clone(), value);
                    },
                    UiMessage::Takeover(tx) => {
                        taken_over = true;
                        status.wipe_out(&mut stderr);
                        let _ = tx.send(());
                    },
                    UiMessage::Release => {
                        taken_over = false;
                        for buf in log_queue.drain(..) {
                            let _ = std::io::Write::write_all(&mut stderr, &buf);
                        }
                        status.write_status(&mut stderr);
                    },
                    UiMessage::Clear => {
                        status.clear(&mut stderr);
                    },
                    UiMessage::LogLine(line) => {
                        if taken_over {
                            log_queue.push_back(line);
                        } else {
                            status.clear(&mut stderr);
                            for buf in log_queue.drain(..) {
                                let _ = std::io::Write::write_all(&mut stderr, &buf);
                            }
                            let _ = std::io::Write::write_all(&mut stderr, &line);
                            status.write_status(&mut stderr);
                        }
                    },
                    UiMessage::ActivityBegin { node, id, activity_category } => {
                        let tracker = status.nix_activities.entry(node).or_default();
                        tracker.begin(id, activity_category);
                    }
                    UiMessage::ActivityEnd { node, id, .. } => {
                        let tracker = status.nix_activities.entry(node).or_default();
                        tracker.end(id);
                    }
                }
            }

            _ = ticker.tick() => {
                if taken_over {
                    continue;
                }

                status.clear(&mut stderr);
                status.write_status(&mut stderr);
            }
        }
    }
}
