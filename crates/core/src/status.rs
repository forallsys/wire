// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

use owo_colors::{OwoColorize, Stream, Style};
use std::{
    collections::VecDeque,
    fmt::Write,
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};
use termion::{clear, cursor, terminal_size};
use tokio::sync::{
    mpsc::{self, UnboundedReceiver},
    oneshot,
};

use crate::hive::node::Name;

use std::collections::HashMap;

pub const BUILD_NAME_STYLE: Style = Style::new().dimmed();
pub const BUILD_NAME_CARET: &str = ">";

// Statuses are ordered deliberately such that failed nodes are at the top of
// the list and Succeeded nodes are at the bottom / never shown.
#[derive(Default, PartialEq, Eq)]
#[repr(u16)]
pub enum NodeStatus {
    Failed,
    Running {
        status: String,
        /// optional previous log with associated build name
        last_log: Option<(String, Option<Arc<String>>)>,
    },
    #[default]
    Pending,
    Succeeded,
}

struct NumStatusCount {
    finished: i32,
    running: i32,
    failed: i32,
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
    /// Writes above the status line, optionally ties it to a specific node
    LogLine(Vec<u8>),
    /// A log line with associated context, including name and possible build
    /// name
    ContextLogLine {
        name: Name,
        build_name: Option<Arc<String>>,
        log: String,
    },
}

pub struct Status {
    statuses: HashMap<Name, NodeStatus>,
    began: Instant,
    show_progress: bool,
    previous_number_lines: usize,
    buffer: String,
}

pub static UI_SENDER: OnceLock<mpsc::UnboundedSender<UiMessage>> = OnceLock::new();
const MAX_NODE_NAME_LENGTH: usize = 20;
const FALLBACK_TERMINAL_ROWS: usize = 24;
const FALLBACK_TERMINAL_COLS: usize = 100;

impl NodeStatus {
    // manually implemented Ord for NodeStatus, since we want to ignore the
    // enum fields in order, as they can rapidly change
    fn discriminant(&self) -> u16 {
        match self {
            Self::Failed => 0,
            Self::Running { .. } => 1,
            Self::Pending => 2,
            Self::Succeeded => 3,
        }
    }

    fn render(&self) -> String {
        match self {
            Self::Pending => "Waiting"
                .if_supports_color(Stream::Stderr, |x| x.dimmed())
                .to_string(),
            Self::Running {
                status,
                last_log: None,
            } => format!(
                "{}",
                status.if_supports_color(Stream::Stderr, |x| x
                    .style(Style::new().blue().on_default_color()))
            ),
            Self::Running {
                status,
                last_log: Some((last_log, None)),
            } => format!(
                "{} {}",
                status.if_supports_color(Stream::Stderr, |x| x
                    .style(Style::new().blue().on_default_color())),
                last_log
            ),
            Self::Running {
                status,
                last_log: Some((last_log, Some(build_name))),
            } => format!(
                "{} {}{} {}",
                status.if_supports_color(Stream::Stderr, |x| x
                    .style(Style::new().blue().on_default_color())),
                build_name.if_supports_color(Stream::Stderr, |x| x.style(BUILD_NAME_STYLE)),
                BUILD_NAME_CARET.if_supports_color(Stream::Stderr, |x| x.style(BUILD_NAME_STYLE)),
                last_log
            ),
            Self::Succeeded => "Succeeded"
                .if_supports_color(Stream::Stderr, |x| x.green())
                .to_string(),
            Self::Failed => "Failed"
                .if_supports_color(Stream::Stderr, |x| x.red())
                .to_string(),
        }
    }
}

impl PartialOrd for NodeStatus {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for NodeStatus {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.discriminant().cmp(&other.discriminant())
    }
}

impl Status {
    fn new() -> Self {
        Self {
            statuses: HashMap::default(),
            began: Instant::now(),
            show_progress: false,
            previous_number_lines: 0,
            buffer: String::with_capacity(1024),
        }
    }

    pub const fn show_progress(&mut self, show_progress: bool) {
        self.show_progress = show_progress;
    }

    fn count_statuses(&self) -> NumStatusCount {
        self.statuses.values().fold(
            NumStatusCount {
                finished: 0,
                running: 0,
                failed: 0,
            },
            |mut count, status| {
                let did_fail = matches!(status, NodeStatus::Failed);
                let is_running = matches!(status, NodeStatus::Running { .. });
                let did_succeeded = matches!(status, NodeStatus::Succeeded | NodeStatus::Failed);

                if did_fail {
                    count.failed += 1;
                }

                if is_running {
                    count.running += 1;
                }

                if did_succeeded || did_fail {
                    count.finished += 1;
                }

                count
            },
        )
    }

    fn get_header(&self) -> String {
        let count = self.count_statuses();

        let mut msg = format!("[{} / {}", count.finished, self.statuses.len());

        let failed = if count.failed >= 1 {
            Some(format!(
                "{} Failed",
                count.failed.if_supports_color(Stream::Stderr, |x| x.red())
            ))
        } else {
            None
        };

        let running = if count.running >= 1 {
            Some(format!(
                "{} Deploying",
                count
                    .running
                    .if_supports_color(Stream::Stderr, |x| x.blue())
            ))
        } else {
            None
        };

        let _ = match (failed, running) {
            (None, None) => write!(&mut msg, ""),
            (Some(message), None) | (None, Some(message)) => write!(&mut msg, " ({message})"),
            (Some(failed), Some(running)) => write!(&mut msg, " ({failed}, {running})"),
        };

        let _ = write!(&mut msg, "]");

        let _ = write!(&mut msg, " {}s", self.began.elapsed().as_secs());

        msg
    }

    /// Sets self.buffer to the current status bar
    pub fn compute_msg(&mut self) {
        self.buffer.clear();

        if self.statuses.is_empty() {
            return;
        }

        let rows = terminal_size().map_or(FALLBACK_TERMINAL_ROWS, |(_, r)| r as usize);
        let cols = terminal_size().map_or(FALLBACK_TERMINAL_COLS, |(c, _)| c as usize);

        let _ = write!(
            self.buffer,
            "{}",
            console::truncate_str(&self.get_header(), cols, "...")
        );

        let mut entries: Vec<(String, &NodeStatus)> = self
            .statuses
            .iter()
            .filter_map(|(name, status)| {
                if matches!(status, NodeStatus::Succeeded) {
                    return None;
                }

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

                Some((truncated, status))
            })
            .collect();

        let max_name_len = entries.iter().map(|(t, _)| t.len()).max().unwrap_or(0);

        // sort by status priority then sort by name
        entries.sort_by(|(na, sa), (nb, sb)| sa.cmp(sb).then_with(|| na.cmp(nb)));

        // cap the displayed node lines to half the terminal height.
        // this keeps the status bar from overflowing.
        let cap = rows.saturating_sub(1).max(1) / 2;

        let mut shown = 0;
        for (truncated, status) in &entries {
            if shown >= cap {
                break;
            }

            let line = format!(
                "\n  {:<node_len$} {}",
                truncated.if_supports_color(Stream::Stderr, |x| x.bold()),
                status.render(),
                node_len = max_name_len.saturating_sub(truncated.len())
            );
            let _ = write!(
                &mut self.buffer,
                "{}",
                console::truncate_str(&line, cols, "...")
            );
            shown += 1;
        }

        let hidden = entries.len().saturating_sub(shown);
        if hidden > 0 {
            let _ = write!(&mut self.buffer, "\n  ... and {hidden} more");
        }
    }

    pub fn clear<T: std::io::Write>(&self, writer: &mut T) {
        if !self.show_progress {
            return;
        }

        for _ in 0..self.previous_number_lines {
            let _ = write!(writer, "\r{}{}", clear::CurrentLine, cursor::Up(1));
        }

        let _ = write!(writer, "\r{}", clear::CurrentLine);
        let _ = writer.flush();
    }

    /// used when there is an interactive prompt
    pub fn wipe_out<T: std::io::Write>(&self, writer: &mut T) {
        if !self.show_progress {
            return;
        }

        self.clear(writer);
    }

    pub fn write_status<T: std::io::Write>(&mut self, writer: &mut T) {
        if !self.show_progress {
            return;
        }

        self.compute_msg();

        let _ = write!(writer, "{}", self.buffer);
        let _ = writer.flush();

        self.previous_number_lines = self.buffer.lines().count().saturating_sub(1);
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
                        status.statuses.extend(
                            names
                                .iter()
                                .map(|name| (name.clone(), NodeStatus::Pending)),
                        );
                    },
                    UiMessage::SetStatus(name, value) => {
                        status.statuses.insert(name.clone(), value);
                    },
                    UiMessage::Takeover(tx) => {
                        taken_over = true;
                        status.wipe_out(&mut stderr);
                        let _ = tx.send(());
                    },
                    UiMessage::Release => {
                        taken_over = false;
                        #[allow(clippy::iter_with_drain)]
                        for buf in log_queue.drain(..) {
                            let _ = std::io::Write::write_all(&mut stderr, &buf);
                        }
                        status.write_status(&mut stderr);
                    },
                    UiMessage::Clear => {
                        status.clear(&mut stderr);
                    },
                    UiMessage::ContextLogLine { name, log, build_name } => {
                        if let Some(node_status) = status.statuses.get_mut(&name) && let NodeStatus::Running { last_log, .. } = node_status {
                            // ensure only a single line is kept or the status
                            // bar might accidentally spread into multiple lines
                            *last_log = log.lines().next().map(|log| (log.to_string(), build_name));
                        }
                    },
                    UiMessage::LogLine(line) => {
                        if taken_over {
                            log_queue.push_back(line);
                        } else {
                            status.clear(&mut stderr);
                            #[allow(clippy::iter_with_drain)]
                        for buf in log_queue.drain(..) {
                                let _ = std::io::Write::write_all(&mut stderr, &buf);
                            }
                            let _ = std::io::Write::write_all(&mut stderr, &line);
                            status.write_status(&mut stderr);
                        }
                    },
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
