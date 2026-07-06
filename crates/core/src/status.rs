// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

use owo_colors::OwoColorize;
use std::{
    collections::VecDeque,
    fmt::Write,
    sync::OnceLock,
    time::{Duration, Instant},
};
use termion::{clear, cursor, terminal_size};
use tokio::sync::{
    mpsc::{self, UnboundedReceiver},
    oneshot,
};

use crate::hive::node::Name;

use std::collections::HashMap;

// Statuses are ordered deliberately such that failed nodes are at the top of
// the list and Succeeded nodes are at the bottom / never shown.
#[derive(Default, PartialEq, PartialOrd, Ord, Eq)]
pub enum NodeStatus {
    Failed,
    Running(String),
    #[default]
    Pending,
    Succeeded,
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
}

pub struct Status {
    statuses: HashMap<Name, NodeStatus>,
    began: Instant,
    show_progress: bool,
    previous_number_lines: usize,
}

pub static UI_SENDER: OnceLock<mpsc::UnboundedSender<UiMessage>> = OnceLock::new();
const MAX_NODE_NAME_LENGTH: usize = 20;
const FALLBACK_TERMINAL_ROWS: usize = 24;

impl Status {
    fn new() -> Self {
        Self {
            statuses: HashMap::default(),
            began: Instant::now(),
            show_progress: false,
            previous_number_lines: 0,
        }
    }

    pub const fn show_progress(&mut self, show_progress: bool) {
        self.show_progress = show_progress;
    }

    #[must_use]
    pub fn get_msg(&self) -> String {
        if self.statuses.is_empty() {
            return String::new();
        }

        let (num_finished, num_running, num_failed) = self.statuses.values().fold(
            (0, 0, 0),
            |(mut finished, mut running, mut failed), status| {
                let did_fail = matches!(status, NodeStatus::Failed);
                let is_running = matches!(status, NodeStatus::Running(..));
                let did_succeeded = matches!(status, NodeStatus::Succeeded | NodeStatus::Failed);

                if did_fail {
                    failed += 1;
                }

                if is_running {
                    running += 1;
                }

                if did_succeeded || did_fail {
                    finished += 1;
                }

                (finished, running, failed)
            },
        );

        let mut msg = format!("[{} / {}", num_finished, self.statuses.len());

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

        let _ = write!(&mut msg, " {}s", self.began.elapsed().as_secs());

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
        let rows = terminal_size().map_or(FALLBACK_TERMINAL_ROWS, |(_, r)| r as usize);
        let cap = rows.saturating_sub(1).max(1) / 2;

        let mut shown = 0;
        for (truncated, status) in &entries {
            if shown >= cap {
                break;
            }

            let line = format!(
                "\n  {}{} {}",
                truncated.bold(),
                " ".repeat(max_name_len.saturating_sub(truncated.len())),
                match status {
                    NodeStatus::Pending => "Waiting".dimmed().to_string(),
                    NodeStatus::Running(task) => {
                        format!("Running {}", task.blue())
                            .on_default_color()
                            .to_string()
                    }
                    NodeStatus::Succeeded => unreachable!("filtered above"),
                    NodeStatus::Failed => "Failed".red().to_string(),
                }
            );
            let _ = write!(&mut msg, "{line}");
            shown += 1;
        }

        let hidden = entries.len().saturating_sub(shown);
        if hidden > 0 {
            let _ = write!(&mut msg, "\n  ... and {hidden} more");
        }

        msg
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

        let msg = self.get_msg();

        let _ = write!(writer, "{msg}");
        let _ = writer.flush();

        self.previous_number_lines = msg.lines().count().saturating_sub(1);
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
