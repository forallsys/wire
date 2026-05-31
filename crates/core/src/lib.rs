// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

#![feature(iter_intersperse)]
#![feature(sync_nonpoison)]
#![feature(nonpoison_mutex)]

use std::{
    collections::{HashMap, HashSet},
    io::IsTerminal,
    ops::Deref,
    process::Stdio,
    sync::{Arc, LazyLock, atomic::AtomicBool, nonpoison::Mutex},
};

use nix_compat::log::LogMessage;
use tokio::{
    process::{ChildStdin, ChildStdout, Command},
    sync::{AcquireError, Semaphore, SemaphorePermit, mpsc::UnboundedSender, oneshot},
};
use tracing::{info, instrument, trace};
use wire_nix_client::{
    NixClient, NixDaemonClientError, WireAddToStoreNarRequest, store_path::SafeStorePath,
};

use crate::{
    commands::trace_nix_log_message,
    errors::HiveLibError,
    hive::node::{Context, Name, Push, SharedTarget},
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
#[allow(clippy::struct_excessive_bools)]
pub struct SubCommandModifiers {
    pub show_trace: bool,
    pub non_interactive: bool,
    pub ssh_accept_host: StrictHostKeyChecking,
    pub ssh_verbosity: usize,
    pub print_build_logs: bool,
    pub experimental_nix_client: bool,
}

impl Default for SubCommandModifiers {
    fn default() -> Self {
        SubCommandModifiers {
            show_trace: false,
            non_interactive: !std::io::stdin().is_terminal(),
            ssh_accept_host: StrictHostKeyChecking::default(),
            ssh_verbosity: 0,
            print_build_logs: false,
            experimental_nix_client: false,
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

#[instrument(skip(trace_callback))]
pub async fn open_remote_client<D, T>(
    target: &D,
    modifiers: SubCommandModifiers,
    trace_callback: T,
    should_quit: Arc<AtomicBool>,
) -> Result<(NixClient<ChildStdout, ChildStdin, T>, String), HiveLibError>
where
    D: Deref<Target = crate::hive::node::Target> + std::fmt::Debug,
    T: Fn(LogMessage, &Arc<Mutex<HashMap<u64, Arc<String>>>>, bool) -> Option<String>,
{
    let mut command = Command::new("ssh")
        .args(target.create_ssh_args(modifiers, true)?)
        .arg(target.get_preferred_host()?.to_string())
        .arg("nix-daemon --stdio")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        // TODO: move to separate thread
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|error| {
            HiveLibError::NixDaemonClientError(NixDaemonClientError::NixDaemonConnectionFailure(
                error,
            ))
        })?;

    let stdin = command.stdin.take().unwrap();
    let stdout = command.stdout.take().unwrap();

    tokio::spawn(async move { command.wait().await });

    Ok((
        NixClient::<ChildStdout, ChildStdin, T>::handshake(
            stdout,
            stdin,
            trace_callback,
            should_quit,
            modifiers.print_build_logs,
        )
        .await
        .map_err(HiveLibError::NixDaemonClientError)?,
        target.get_preferred_host()?.to_string(),
    ))
}

fn get_common_copy_path_help(error: &NixDaemonClientError) -> Option<String> {
    if let NixDaemonClientError::NixDaemonOperationError { msg, .. } = error
        && (msg.contains("error: unexpected end-of-file"))
    {
        Some("wire requires the deploying user or wire binary cache is trusted on the remote server. if you're attempting to make that change, skip keys with --no-keys. please read https://wire.forall.systems/guides/keys for more information".to_string())
    } else {
        None
    }
}

/// Pushes the path with a native daemon.
pub async fn push_with_daemon(
    context: &Context,
    target: &SharedTarget,
    push: Push<'_>,
    substitute_on_destination: bool,
) -> Result<(), HiveLibError> {
    let mut local_daemon = NixClient::open_local(
        trace_nix_log_message,
        context.should_quit.clone(),
        context.modifiers.print_build_logs,
    )
    .await
    .map_err(HiveLibError::NixDaemonClientError)?;

    let target = target.0.read().await;

    let (mut remote_daemon, host) = open_remote_client(
        &target,
        context.modifiers,
        trace_nix_log_message,
        context.should_quit.clone(),
    )
    .await?;

    let path = match push {
        Push::Derivation(path) | Push::Path(path) => path.clone(),
    };

    info!(path = ?path, "attempting copy");

    let closure = local_daemon
        .collect_complete_closure(&path)
        .await
        .map_err(HiveLibError::NixDaemonClientError)?;
    let closure_length = closure.len();

    info!(path = ?path, "closure has {:?} paths", closure_length);

    let paths_on_target: HashSet<_> = remote_daemon
        .query_valid_paths(closure.clone(), substitute_on_destination)
        .await
        .map_err(HiveLibError::NixDaemonClientError)?
        .into_iter()
        .collect();

    trace!(path = ?path, "target already has {} path(s)", paths_on_target.len());

    let paths_to_push = closure_length.saturating_sub(paths_on_target.len());
    if paths_to_push > 0 {
        info!("pushing {}", closure_length - paths_on_target.len());
    }

    let paths_to_upload = closure.into_iter().filter(|p| !paths_on_target.contains(p));

    for path in paths_to_upload {
        info!("copying '{}' to node {host}", path.to_absolute_path());

        let Some(path_info) =
            local_daemon
                .query(&path)
                .await
                .map_err(|err| HiveLibError::NixCopyError {
                    name: context.name.clone(),
                    path: path.clone(),
                    help: get_common_copy_path_help(&err),
                    error: Box::new(err),
                })?
        else {
            return Err(HiveLibError::NixCopyError {
                name: context.name.clone(),
                path: path.clone(),
                error: Box::new(NixDaemonClientError::NixDaemonOperationFailed(format!(
                    "selected {path:?} for upload does not exist in local store"
                ))),
                help: None,
            });
        };

        let nar_stream = local_daemon
            .get_nar_stream(&path, path_info.nar_size)
            .await
            .map_err(|err| HiveLibError::NixCopyError {
                name: context.name.clone(),
                path: path.clone(),
                help: get_common_copy_path_help(&err),
                error: Box::new(err),
            })?;

        remote_daemon
            .add_to_store_nar(
                WireAddToStoreNarRequest {
                    path: path.clone(),
                    deriver: path_info.deriver.map(Into::into),
                    nar_hash: path_info.nar_hash,
                    references: path_info
                        .references
                        .into_iter()
                        .map(SafeStorePath)
                        .collect(),
                    registration_time: path_info.registration_time,
                    nar_size: path_info.nar_size,
                    ultimate: false,
                    signatures: path_info.signatures,
                    ca: path_info.ca,
                    repair: false,
                    dont_check_sigs: true,
                },
                nar_stream,
            )
            .await
            .map_err(|err| HiveLibError::NixCopyError {
                name: context.name.clone(),
                path: path.clone(),
                help: get_common_copy_path_help(&err),
                error: Box::new(err),
            })?;
    }

    Ok(())
}
