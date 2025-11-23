// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

use owo_colors::OwoColorize;
use std::{fmt::Write, time::Instant};
use termion::{clear, cursor};

use crate::{STDIN_CLOBBER_LOCK, hive::node::Name};

use std::{
    collections::HashMap,
    sync::{LazyLock, nonpoison::Mutex},
};

#[derive(Default)]
pub enum NodeStatus {
    #[default]
    Pending,
    Running(String),
    Succeeded,
    Failed,
}

pub struct Status {
    statuses: HashMap<String, NodeStatus>,
    began: Instant,
    show_progress: bool,
}

/// global status used for the progress bar in the cli crate
pub static STATUS: LazyLock<Mutex<Status>> = LazyLock::new(|| Mutex::new(Status::new()));

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

    pub fn add_many(&mut self, names: &[&Name]) {
        self.statuses.extend(
            names
                .iter()
                .map(|name| (name.0.to_string(), NodeStatus::Pending)),
        );
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

    pub fn write_above_status<T: std::io::Write>(
        &mut self,
        buf: &[u8],
        writer: &mut T,
    ) -> std::io::Result<usize> {
        if STDIN_CLOBBER_LOCK.available_permits() != 1 {
            // skip
            return Ok(0);
        }

        self.clear(writer);
        let written = writer.write(buf)?;
        self.write_status(writer);

        Ok(written)
    }
}
