// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

#![feature(assert_matches)]
#![feature(iter_intersperse)]
#![feature(sync_nonpoison)]
#![feature(nonpoison_mutex)]

use std::{io::IsTerminal, sync::LazyLock};

use tokio::sync::{AcquireError, Semaphore, SemaphorePermit, mpsc::UnboundedSender, oneshot};

use crate::{
    errors::HiveLibError,
    hive::node::Name,
    status::{UI_SENDER, UiMessage},
};

pub mod cache;
pub mod commands;
pub mod hive;
pub mod status;

#[cfg(test)]
mod test_macros;

#[cfg(test)]
mod test_support;

pub mod errors;

#[derive(Clone, Debug, Copy, Default)]
pub enum StrictHostKeyChecking {
    /// do not accept new host. dangerous!
    No,

    /// accept-new, default
    #[default]
    AcceptNew,
}

#[derive(Debug, Clone, Copy)]
pub struct SubCommandModifiers {
    pub show_trace: bool,
    pub non_interactive: bool,
    pub ssh_accept_host: StrictHostKeyChecking,
    pub ssh_verbosity: usize,
}

impl Default for SubCommandModifiers {
    fn default() -> Self {
        SubCommandModifiers {
            show_trace: false,
            non_interactive: !std::io::stdin().is_terminal(),
            ssh_accept_host: StrictHostKeyChecking::default(),
            ssh_verbosity: 0,
        }
    }
}

pub enum EvalGoal<'a> {
    Inspect,
    Names,
    GetTopLevel(&'a Name),
}

pub static STDIN_CLOBBER_LOCK: LazyLock<Semaphore> = LazyLock::new(|| Semaphore::new(1));

/// `SemaphorePermit` that sends a `UiMessage::Release` on drop
pub struct ClobberGuard<'a>(
    #[allow(unused)] SemaphorePermit<'a>,
    Option<&'a UnboundedSender<UiMessage>>,
);

impl Drop for ClobberGuard<'_> {
    fn drop(&mut self) {
        if let Some(tx) = self.1 {
            let _ = tx.send(UiMessage::Release);
        }
    }
}

pub async fn acquire_stdin_lock<'a>() -> Result<ClobberGuard<'a>, AcquireError> {
    let result = STDIN_CLOBBER_LOCK.acquire().await?;
    let (sender, rx) = oneshot::channel();
    let tx = UI_SENDER.get();

    if let Some(tx) = tx {
        let _ = tx.send(UiMessage::Takeover(sender));

        // wait until takeover is confirmed
        let _ = rx.await;
    }

    let result = ClobberGuard(result, tx);

    Ok(result)
}
