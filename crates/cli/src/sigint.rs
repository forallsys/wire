// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

use std::sync::{Arc, atomic::AtomicBool};

use signal_hook::consts::SIGINT;
use signal_hook_tokio::Signals;

use futures::stream::StreamExt;
use tracing::info;

pub(crate) async fn handle_signals(mut signals: Signals, should_shutdown: Arc<AtomicBool>) {
    while let Some(signal) = signals.next().await {
        if let SIGINT = signal
            && !should_shutdown.load(std::sync::atomic::Ordering::Relaxed)
        {
            info!("Received SIGINT, attempting to shut down executor tasks.");
            should_shutdown.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
}
