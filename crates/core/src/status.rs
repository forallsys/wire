// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

use crate::hive::node::Name;
use nix_compat::log::{ActivityType, ResultType};
use owo_colors::OwoColorize;
use std::{
    collections::VecDeque,
    fmt::Write,
    sync::OnceLock,
    time::{Duration, Instant},
};
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
        activity_type: ActivityType,
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
    Build,
    BuildPending,
}

impl TryFrom<ActivityType> for ActivityCategory {
    type Error = ();

    fn try_from(activity_type: ActivityType) -> Result<Self, Self::Error> {
        Ok(match activity_type {
            ActivityType::CopyPath
            | ActivityType::FileTransfer
            | ActivityType::Substitute
            | ActivityType::QueryPathInfo
            | ActivityType::FetchTree
            | ActivityType::CopyPaths => Self::Download,

            ActivityType::Build | ActivityType::Builds => Self::Build,

            ActivityType::BuildWaiting => Self::BuildPending,

            _ => return Err(()),
        })
    }
}

#[derive(Default, Debug)]
pub struct ActivityTracker {
    active_by_id: HashMap<u64, ActivityCategory>,

    pub active_downloads: usize,
    pub completed_downloads: usize,

    pub active_builds: usize,
    pub pending_builds: usize,
    pub completed_builds: usize,
}

impl ActivityTracker {
    pub fn begin(&mut self, id: u64, category: ActivityType) {
        let Ok(category) = category.try_into() else {
            return;
        };

        self.active_by_id.insert(id, category);

        match category {
            ActivityCategory::Download => self.active_downloads += 1,
            ActivityCategory::Build => self.active_builds += 1,
            ActivityCategory::BuildPending => self.pending_builds += 1,
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
            }
        }
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

        let _ = write!(&mut msg, "{}s Elapsed ", self.began.elapsed().as_secs());

        let _ = write!(
            &mut msg,
            "[Nodes {} / {}",
            self.num_finished(),
            self.node_statuses.len()
        );

        let num_failed = self.num_failed();
        let num_running = self.num_running();

        let failed = if num_failed >= 1 {
            Some(format!("{} Failed", num_failed.red()))
        } else {
            None
        };

        let running = if num_running >= 1 {
            Some(format!("{} Deploying", num_running.blue()))
        } else {
            None
        };

        let _ = match (failed, running) {
            (None, None) => write!(&mut msg, ""),
            (Some(message), None) | (None, Some(message)) => write!(&mut msg, " ({message})"),
            (Some(failed), Some(running)) => write!(&mut msg, " ({failed}, {running})"),
        };

        let _ = write!(&mut msg, "]");

        // let (total_active_downloads, total_completed_downloads) =
        //     self.nix_activities
        //         .iter()
        //         .fold((0, 0), |acc, (_, tracker)| {
        //             (
        //                 acc.0 + tracker.active_downloads,
        //                 acc.1 + tracker.completed_downloads,
        //             )
        //         });
        //
        // let (total_active_builds, total_completed_builds, total_pending_builds) =
        //     self.nix_activities
        //         .iter()
        //         .fold((0, 0, 0), |acc, (_, tracker)| {
        //             (
        //                 acc.0 + tracker.active_builds,
        //                 acc.1 + tracker.completed_builds,
        //                 acc.2 + tracker.pending_builds,
        //             )
        //         });
        //
        // if total_active_downloads > 0 || total_completed_downloads > 0 {
        //     let _ = write!(
        //         &mut msg,
        //         " [DL Jobs: {total_active_downloads} active, {total_completed_downloads} done]",
        //     );
        // }
        //
        // if total_active_builds > 0 || total_completed_builds > 0 {
        //     let _ = write!(
        //         &mut msg,
        //         " [Build Jobs: {total_pending_builds} pending, {total_active_builds} active, {total_completed_builds} done]",
        //     );
        // }

        let node_name_strings = self
            .node_statuses
            .iter()
            .map(|(name, status)| {
                (
                    name,
                    format!(
                        "\n  {} {}",
                        name.bold(),
                        match status {
                            NodeStatus::Pending => "Waiting".to_string().dimmed().to_string(),
                            NodeStatus::Running(task) => format!("Running {}", task.blue())
                                .on_default_color()
                                .to_string(),
                            NodeStatus::Succeeded => "Finished".to_string().green().to_string(),
                            NodeStatus::Failed => "Failed".to_string().red().to_string(),
                        },
                    )
                    .to_string(),
                )
            })
            .collect::<HashMap<_, _>>();

        let node_status_strings = self
            .nix_activities
            .iter()
            .filter_map(|(name, tracker)| {
                name.as_ref().map(|name| {
                    (
                        name,
                        format!(
                            " {} ❘ {} ❘ {} ❘ {} ❘ {}",
                            tracker.active_builds.yellow(),
                            tracker.completed_builds,
                            tracker.pending_builds.blue(),
                            tracker.active_downloads.yellow(),
                            tracker.completed_downloads.green()
                        )
                        .to_string(),
                    )
                })
            })
            .collect::<HashMap<_, _>>();

        for (name, name_string) in node_name_strings {
            let status_string = node_status_strings.get(&name.clone());

            let _ = write!(
                &mut msg,
                "{name_string}{}",
                match status_string {
                    Some(string) => string.as_str(),
                    None => "",
                }
            );
        }

        msg
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
                    UiMessage::ActivityBegin { node, id, activity_type } => {
                        let tracker = status.nix_activities.entry(node).or_default();
                        tracker.begin(id, activity_type);
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
