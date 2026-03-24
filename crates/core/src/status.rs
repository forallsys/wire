// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

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

use crate::hive::node::Name;

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
}

pub struct Status {
    statuses: HashMap<String, NodeStatus>,
    began: Instant,
    show_progress: bool,
}

pub static UI_SENDER: OnceLock<mpsc::UnboundedSender<UiMessage>> = OnceLock::new();

impl Status {
    fn new() -> Self {
        Self {
            statuses: HashMap::default(),
            began: Instant::now(),
            show_progress: false,
        }
    }

    pub const fn show_progress(&mut self, show_progress: bool) {
        self.show_progress = show_progress;
    }

    pub fn set_node_step(&mut self, node: &Name, step: String) {
        self.statuses
            .insert(node.0.to_string(), NodeStatus::Running(step));
    }

    pub fn mark_node_failed(&mut self, node: &Name) {
        self.statuses.insert(node.0.to_string(), NodeStatus::Failed);
    }

    pub fn mark_node_succeeded(&mut self, node: &Name) {
        self.statuses
            .insert(node.0.to_string(), NodeStatus::Succeeded);
    }

    #[must_use]
    fn num_finished(&self) -> usize {
        self.statuses
            .iter()
            .filter(|(_, status)| matches!(status, NodeStatus::Succeeded | NodeStatus::Failed))
            .count()
    }

    #[must_use]
    fn num_running(&self) -> usize {
        self.statuses
            .iter()
            .filter(|(_, status)| matches!(status, NodeStatus::Running(..)))
            .count()
    }

    #[must_use]
    fn num_failed(&self) -> usize {
        self.statuses
            .iter()
            .filter(|(_, status)| matches!(status, NodeStatus::Failed))
            .count()
    }

    #[must_use]
    pub fn get_msg(&self) -> String {
        if self.statuses.is_empty() {
            return String::new();
        }

        let mut msg = format!("[{} / {}", self.num_finished(), self.statuses.len(),);

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

        let _ = write!(&mut msg, " {}s", self.began.elapsed().as_secs());

        msg
    }

    pub fn clear<T: std::io::Write>(&self, writer: &mut T) {
        if !self.show_progress {
            return;
        }

        let _ = write!(writer, "{}", cursor::Save);
        // let _ = write!(writer, "{}", cursor::Down(1));
        let _ = write!(writer, "{}", cursor::Left(999));
        let _ = write!(writer, "{}", clear::CurrentLine);
    }

    /// used when there is an interactive prompt
    pub fn wipe_out<T: std::io::Write>(&self, writer: &mut T) {
        if !self.show_progress {
            return;
        }

        let _ = write!(writer, "{}", cursor::Save);
        let _ = write!(writer, "{}", cursor::Left(999));
        let _ = write!(writer, "{}", clear::CurrentLine);
        let _ = writer.flush();
    }

    pub fn write_status<T: std::io::Write>(&mut self, writer: &mut T) {
        if self.show_progress {
            let _ = write!(writer, "{}", self.get_msg());
        }
    }
}

pub async fn status_tick_worker(mut rx: UnboundedReceiver<UiMessage>, show_progress: bool) {
    let mut status = Status::new();

    status.show_progress(show_progress);

    let mut ticker = tokio::time::interval(Duration::from_secs(1));
    let mut stderr = std::io::stderr();
    let mut log_queue: VecDeque<Vec<u8>> = VecDeque::with_capacity(100);
    let mut taken_over = false;

    loop {
        tokio::select! {
            Some(msg) = rx.recv() => {
                match msg {
                    UiMessage::AddMany(names) => {
                        status.statuses.extend(
                            names
                                .iter()
                                .map(|name| (name.0.to_string(), NodeStatus::Pending)),
                        );
                    },
                    UiMessage::SetStatus(name, value) => {
                        status.statuses.insert(name.0.to_string(), value);
                    },
                    UiMessage::Takeover(tx) => {
                        taken_over = true;
                        status.wipe_out(&mut stderr);
                        let _ = tx.send(());
                    },
                    UiMessage::Release => {
                        taken_over = false;
                        for buf in log_queue.drain(..) {
                            let _ = std::io::Write::write(&mut stderr, &buf).map(|_| ());
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
                                let _ = std::io::Write::write(&mut stderr, &buf).map(|_| ());
                            }
                            let _ = std::io::Write::write(&mut stderr, &line);
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
