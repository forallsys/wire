// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

use crate::{
    commands::pty::{InteractiveChildChip, interactive_command_with_env},
    hive::node::{Name, SharedTarget},
    status::UI_SENDER,
};
use std::{
    collections::HashMap,
    sync::{Arc, LazyLock},
};

use aho_corasick::AhoCorasick;
use itertools::Itertools;
use nix_compat::log::{AT_NIX_PREFIX, LogMessage, VerbosityLevel};
use tracing::{debug, error, info, trace, warn};

use crate::{
    SubCommandModifiers,
    commands::noninteractive::{NonInteractiveChildChip, non_interactive_command_with_env},
    errors::{CommandError, HiveLibError},
};

pub(crate) mod builder;
pub mod common;
pub(crate) mod noninteractive;
pub(crate) mod pty;

#[derive(Clone, Debug)]
pub(crate) enum ChildOutputMode {
    Nix(Option<Name>),
    Generic,
    Interactive,
}

#[derive(Debug)]
pub enum Either<L, R> {
    Left(L),
    Right(R),
}

#[derive(Debug)]
pub(crate) struct CommandArguments<S: AsRef<str>> {
    modifiers: SubCommandModifiers,
    target: Option<SharedTarget>,
    output_mode: ChildOutputMode,
    command_string: S,
    keep_stdin_open: bool,
    privilege_escalation_command: Option<String>,
    log_stdout: bool,
}

static AHO_CORASICK: LazyLock<AhoCorasick> = LazyLock::new(|| {
    AhoCorasick::builder()
        .ascii_case_insensitive(false)
        .match_kind(aho_corasick::MatchKind::LeftmostFirst)
        .build([AT_NIX_PREFIX])
        .unwrap()
});

impl<S: AsRef<str>> CommandArguments<S> {
    pub(crate) const fn new(command_string: S, modifiers: SubCommandModifiers) -> Self {
        Self {
            command_string,
            keep_stdin_open: false,
            privilege_escalation_command: None,
            log_stdout: false,
            target: None,
            output_mode: ChildOutputMode::Generic,
            modifiers,
        }
    }

    pub(crate) fn execute_on_remote(mut self, target: Option<SharedTarget>) -> Self {
        self.target = target;
        self
    }

    pub(crate) fn mode(mut self, mode: ChildOutputMode) -> Self {
        self.output_mode = mode;
        self
    }

    pub(crate) const fn keep_stdin_open(mut self) -> Self {
        self.keep_stdin_open = true;
        self
    }

    pub(crate) fn privileged(mut self, escalation_command: &[Arc<str>]) -> Self {
        self.privilege_escalation_command = Some(escalation_command.iter().join(" "));
        self
    }

    pub(crate) const fn is_elevated(&self) -> bool {
        self.privilege_escalation_command.is_some()
    }

    pub(crate) const fn log_stdout(mut self) -> Self {
        self.log_stdout = true;
        self
    }
}

pub(crate) async fn run_command<S: AsRef<str>>(
    arguments: &CommandArguments<S>,
) -> Result<Either<InteractiveChildChip, NonInteractiveChildChip>, HiveLibError> {
    run_command_with_env(arguments, HashMap::new()).await
}

pub(crate) async fn run_command_with_env<S: AsRef<str>>(
    arguments: &CommandArguments<S>,
    envs: HashMap<String, String>,
) -> Result<Either<InteractiveChildChip, NonInteractiveChildChip>, HiveLibError> {
    // use the non interactive command runner when forced
    // ... or when there is no reason for interactivity, local and unprivileged
    if arguments.modifiers.non_interactive
        || (arguments.target.is_none() && !arguments.is_elevated())
    {
        return Ok(Either::Right(
            non_interactive_command_with_env(arguments, envs).await?,
        ));
    }

    Ok(Either::Left(
        interactive_command_with_env(arguments, envs).await?,
    ))
}

pub(crate) trait WireCommandChip {
    type ExitStatus;

    async fn wait_till_success(self) -> Result<Self::ExitStatus, CommandError>;
    async fn write_stdin(&mut self, data: Vec<u8>) -> Result<(), HiveLibError>;
}

type ExitStatus = Either<(portable_pty::ExitStatus, String), (std::process::ExitStatus, String)>;

impl WireCommandChip for Either<InteractiveChildChip, NonInteractiveChildChip> {
    type ExitStatus = ExitStatus;

    async fn write_stdin(&mut self, data: Vec<u8>) -> Result<(), HiveLibError> {
        match self {
            Self::Left(left) => left.write_stdin(data).await,
            Self::Right(right) => right.write_stdin(data).await,
        }
    }

    async fn wait_till_success(self) -> Result<Self::ExitStatus, CommandError> {
        match self {
            Self::Left(left) => left.wait_till_success().await.map(Either::Left),
            Self::Right(right) => right.wait_till_success().await.map(Either::Right),
        }
    }
}

impl ChildOutputMode {
    /// this function is by far the biggest hotspot in the whole tree
    /// Returns a string if this log is notable to be stored as an error message
    fn trace_slice(&self, line: &mut [u8]) -> Option<String> {
        let (slice, task_name) = match self {
            Self::Generic | Self::Interactive => {
                let string = String::from_utf8_lossy(line);
                let stripped = strip_ansi_escapes::strip_str(&string);
                warn!("{stripped}");
                return Some(string.to_string());
            }
            Self::Nix(task_name) => {
                let position = AHO_CORASICK.find(&line).map(|x| &mut line[x.end()..]);

                if let Some(json_buf) = position {
                    (json_buf, task_name)
                } else {
                    // usually happens when ssh is outputting something
                    warn!("{}", String::from_utf8_lossy(line));
                    return None;
                }
            }
        };

        let Ok(log_message) = serde_json::from_slice::<LogMessage>(slice) else {
            // failed to parse, print the string regardless as a backup
            warn!("{}", String::from_utf8_lossy(slice));
            return None;
        };

        let (msg, level) = match log_message {
            LogMessage::Start {
                text,
                level,
                id,
                r#type,
                ..
            } => {
                if let Some(tx) = UI_SENDER.get() {
                    let _ = tx.send(crate::status::UiMessage::ActivityBegin(
                        task_name.clone(),
                        id,
                        r#type,
                    ));
                }

                (text, level)
            }
            LogMessage::Stop { id } => {
                if let Some(tx) = UI_SENDER.get() {
                    let _ = tx.send(crate::status::UiMessage::ActivityEnd(
                        task_name.clone(),
                        id,
                        None,
                    ));
                }

                return None;
            }
            LogMessage::Result { id, r#type, .. } => {
                if let Some(tx) = UI_SENDER.get() {
                    let _ = tx.send(crate::status::UiMessage::ActivityEnd(
                        task_name.clone(),
                        id,
                        Some(r#type),
                    ));
                }

                return None;
            }
            LogMessage::Msg { msg, level, .. } => (msg, level),
            LogMessage::SetPhase { .. } => return None,
        };

        if msg.is_empty() {
            return None;
        }

        let msg = strip_ansi_escapes::strip_str(msg);

        match level {
            VerbosityLevel::Info => info!("{msg}"),
            VerbosityLevel::Warn | VerbosityLevel::Notice => warn!("{msg}"),
            VerbosityLevel::Error => error!("{msg}"),
            VerbosityLevel::Debug => debug!("{msg}"),
            VerbosityLevel::Vomit | VerbosityLevel::Talkative | VerbosityLevel::Chatty => {
                trace!("{msg}");
            }
        }

        if matches!(
            level,
            VerbosityLevel::Error | VerbosityLevel::Warn | VerbosityLevel::Notice
        ) {
            return Some(msg);
        }

        None
    }
}
