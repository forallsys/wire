// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

use std::{
    collections::{HashMap, VecDeque},
    process::ExitStatus,
    sync::Arc,
};

use crate::{
    SubCommandModifiers,
    commands::{ChildOutputMode, CommandArguments, WireCommandChip},
    errors::{CommandError, HiveLibError},
    hive::node::{BuildNameMap, SharedTarget},
};
use itertools::Itertools;
use tokio::{
    io::{AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
    sync::Mutex,
    task::JoinSet,
};
use tracing::{Instrument, Level, debug, instrument, trace};

pub struct NonInteractiveChildChip {
    error_collection: Arc<Mutex<VecDeque<String>>>,
    stdout_collection: Arc<Mutex<VecDeque<String>>>,
    child: Child,
    joinset: JoinSet<()>,
    original_command: String,
    stdin: ChildStdin,
}

#[instrument(skip_all, level = Level::DEBUG, name = "run", fields(elevated = %arguments.is_elevated()))]
pub async fn non_interactive_command_with_env<S: AsRef<str> + Sync>(
    arguments: &CommandArguments<S>,
    envs: HashMap<String, String>,
) -> Result<NonInteractiveChildChip, HiveLibError> {
    let mut command = if let Some(ref target) = arguments.target {
        create_sync_ssh_command(target, arguments.modifiers).await?
    } else {
        let mut command = Command::new("bash");

        command.arg("-c");

        command
    };

    let command_string = format!(
        "{command_string}{extra}",
        command_string = arguments.command_string.as_ref(),
        extra = match arguments.output_mode {
            ChildOutputMode::Generic => "",
            ChildOutputMode::Nix(..) => " --log-format internal-json",
        }
    );

    let command_string = command_string.replace('\'', "'\''");

    let command_string = arguments.privilege_escalation_command.as_ref().map_or_else(
        || format!("bash -c '{command_string}'"),
        |escalation_command| format!("{escalation_command} bash -c '{command_string}'"),
    );

    debug!("{command_string}");

    command.arg(&command_string);
    command.stdin(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());
    command.stdout(std::process::Stdio::piped());
    command.kill_on_drop(true);
    // command.env_clear();
    command.envs(envs);

    let mut child = command.spawn().unwrap();
    let error_collection = Arc::new(Mutex::new(VecDeque::<String>::with_capacity(10)));
    let stdout_collection = Arc::new(Mutex::new(VecDeque::<String>::with_capacity(10)));
    let stdin = child.stdin.take().unwrap();

    let stdout_handle = child
        .stdout
        .take()
        .ok_or(HiveLibError::CommandError(CommandError::NoHandle))?;
    let stderr_handle = child
        .stderr
        .take()
        .ok_or(HiveLibError::CommandError(CommandError::NoHandle))?;

    let mut joinset = JoinSet::new();
    let output_mode = Arc::new(arguments.output_mode.clone());

    joinset.spawn(
        handle_io(
            stderr_handle,
            output_mode.clone(),
            error_collection.clone(),
            true,
            true,
            arguments.build_name_map.clone(),
            arguments.modifiers.print_build_logs,
        )
        .in_current_span(),
    );
    joinset.spawn(
        handle_io(
            stdout_handle,
            output_mode,
            stdout_collection.clone(),
            false,
            arguments.log_stdout,
            arguments.build_name_map.clone(),
            arguments.modifiers.print_build_logs,
        )
        .in_current_span(),
    );

    Ok(NonInteractiveChildChip {
        error_collection,
        stdout_collection,
        child,
        joinset,
        original_command: arguments.command_string.as_ref().to_string(),
        stdin,
    })
}

impl WireCommandChip for NonInteractiveChildChip {
    type ExitStatus = (ExitStatus, String);

    async fn wait_till_success(mut self) -> Result<Self::ExitStatus, CommandError> {
        let status = self.child.wait().await.unwrap();
        let _ = self.joinset.join_all().await;

        if !status.success() {
            let logs = self.error_collection.lock().await.iter().rev().join("\n");

            return Err(CommandError::CommandFailed {
                command_ran: self.original_command,
                logs,
                code: status
                    .code()
                    .map_or_else(|| "no exit code".to_string(), |code| format!("code {code}")),
                reason: "known-status",
            });
        }

        let stdout = self.stdout_collection.lock().await.iter().rev().join("\n");

        Ok((status, stdout))
    }

    async fn write_stdin(&mut self, data: Vec<u8>) -> Result<(), HiveLibError> {
        trace!("Writing {} bytes", data.len());
        self.stdin.write_all(&data).await.unwrap();
        Ok(())
    }
}

#[instrument(skip_all, name = "log")]
pub async fn handle_io<R>(
    reader: R,
    output_mode: Arc<ChildOutputMode>,
    collection: Arc<Mutex<VecDeque<String>>>,
    is_error: bool,
    should_log: bool,
    build_name_map: BuildNameMap,
    print_build_logs: bool,
) where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut io_reader = tokio::io::AsyncBufReadExt::lines(BufReader::new(reader));

    while let Some(line) = io_reader.next_line().await.unwrap() {
        let mut line = line.into_bytes();

        let log = if should_log {
            Some(output_mode.as_ref().clone().trace_slice(
                &mut line,
                &build_name_map,
                print_build_logs,
            ))
        } else {
            None
        };

        if !is_error {
            let mut queue = collection.lock().await;
            queue.push_front(String::from_utf8_lossy(&line).to_string());
        } else if let Some(error_msg) = log.flatten() {
            let mut queue = collection.lock().await;
            queue.push_front(error_msg);
            // add at most 20 message to the front, drop the rest.
            queue.truncate(20);
        }
    }

    debug!("io_handler: goodbye!");
}

#[allow(clippy::significant_drop_tightening)]
async fn create_sync_ssh_command(
    target: &SharedTarget,
    modifiers: SubCommandModifiers,
) -> Result<Command, HiveLibError> {
    let target = target.0.read().await;
    let mut command = Command::new("ssh");
    command.args(target.create_ssh_args(modifiers, false)?);
    command.arg(target.get_preferred_host()?.to_string());

    Ok(command)
}
