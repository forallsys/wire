// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

use crate::{
    commands::pty::{InteractiveChildChip, interactive_command_with_env},
    hive::node::{BuildNameMap, SharedTarget},
};
use core::str;
use std::{
    borrow::Cow,
    collections::HashMap,
    path::Path,
    sync::{Arc, LazyLock, nonpoison::Mutex},
};

use aho_corasick::AhoCorasick;
use itertools::Itertools;
use nix_compat::log::{AT_NIX_PREFIX, Field, LogMessage, ResultType, VerbosityLevel};
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

#[derive(Copy, Clone, Debug)]
pub(crate) enum ChildOutputMode {
    Nix,
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
    build_name_map: BuildNameMap,
}

static AHO_CORASICK: LazyLock<AhoCorasick> = LazyLock::new(|| {
    AhoCorasick::builder()
        .ascii_case_insensitive(false)
        .match_kind(aho_corasick::MatchKind::LeftmostFirst)
        .build([AT_NIX_PREFIX])
        .unwrap()
});

impl<S: AsRef<str>> CommandArguments<S> {
    pub(crate) fn new(command_string: S, modifiers: SubCommandModifiers) -> Self {
        Self {
            command_string,
            keep_stdin_open: false,
            privilege_escalation_command: None,
            log_stdout: false,
            target: None,
            output_mode: ChildOutputMode::Generic,
            modifiers,
            build_name_map: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub(crate) fn execute_on_remote(mut self, target: Option<SharedTarget>) -> Self {
        self.target = target;
        self
    }

    pub(crate) const fn mode(mut self, mode: ChildOutputMode) -> Self {
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
    fn trace_slice(self, line: &mut [u8], build_name_map: &BuildNameMap) -> Option<String> {
        let slice = match self {
            Self::Generic | Self::Interactive => {
                let string = String::from_utf8_lossy(line);
                let stripped = strip_ansi_escapes::strip_str(&string);
                warn!("{stripped}");
                return Some(string.to_string());
            }
            Self::Nix => {
                let position = AHO_CORASICK.find(&line).map(|x| &mut line[x.end()..]);

                if let Some(json_buf) = position {
                    json_buf
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

        let (msg, level, build_name) = match log_message {
            LogMessage::Start {
                text,
                r#type,
                level,
                id,
                fields,
                ..
            } => {
                if let Some(fields) = fields
                    && matches!(r#type, nix_compat::log::ActivityType::Build)
                    // first field of start log contains the build name.
                    && let Some(Field::String(name)) = fields.first()
                {
                    build_name_map
                        .lock()
                        .insert(id, drv_path_to_build_name(name));
                }

                (text, level, None)
            }
            LogMessage::Stop { id, .. } => {
                build_name_map.lock().remove(&id);

                return None;
            }
            LogMessage::Msg { msg, level, .. } => (msg, level, None),
            LogMessage::Result {
                r#type: ResultType::BuildLogLine,
                fields,
                id,
                ..
            } => {
                let Some(Field::String(msg)) = fields.into_iter().next() else {
                    return None;
                };

                // Attempt to reuse owned bytes into a utf8 string, or falls
                // back lossy if it fails.
                let msg = match msg {
                    std::borrow::Cow::Borrowed(bytes) => String::from_utf8_lossy(bytes),
                    std::borrow::Cow::Owned(vec) => match String::from_utf8(vec) {
                        Ok(s) => Cow::Owned(s),
                        Err(e) => Cow::Owned(String::from_utf8_lossy(e.as_bytes()).into_owned()),
                    },
                };

                let lock = build_name_map.lock();
                let build_name = lock.get(&id).cloned();

                (msg, VerbosityLevel::Info, build_name)
            }
            _ => return None,
        };

        if msg.is_empty() {
            return None;
        }

        let msg = strip_ansi_escapes::strip_str(msg);

        let level = log_print(&level, build_name.as_ref(), &msg);

        if matches!(level, tracing::Level::ERROR | tracing::Level::WARN) {
            return Some(msg);
        }

        None
    }
}

fn log_print(
    level: &VerbosityLevel,
    build_name: Option<&Arc<String>>,
    msg: &String,
) -> tracing::Level {
    let level: tracing::Level = match level {
        VerbosityLevel::Info => tracing::Level::INFO,
        VerbosityLevel::Warn | VerbosityLevel::Notice => tracing::Level::WARN,
        VerbosityLevel::Error => tracing::Level::ERROR,
        VerbosityLevel::Debug => tracing::Level::DEBUG,
        VerbosityLevel::Vomit | VerbosityLevel::Talkative | VerbosityLevel::Chatty => {
            tracing::Level::TRACE
        }
    };

    if let Some(build_name) = build_name {
        match level {
            tracing::Level::ERROR => error!(build = %build_name, "{msg}"),
            tracing::Level::WARN => warn!(build = %build_name, "{msg}"),
            tracing::Level::INFO => info!(build = %build_name, "{msg}"),
            tracing::Level::DEBUG => debug!(build = %build_name, "{msg}"),
            tracing::Level::TRACE => trace!(build = %build_name, "{msg}"),
        }
    } else {
        match level {
            tracing::Level::ERROR => error!("{msg}"),
            tracing::Level::WARN => warn!("{msg}"),
            tracing::Level::INFO => info!("{msg}"),
            tracing::Level::DEBUG => debug!("{msg}"),
            tracing::Level::TRACE => trace!("{msg}"),
        }
    }

    level
}

fn drv_path_to_build_name(drv_path: &[u8]) -> Arc<String> {
    let string = match String::from_utf8(drv_path.to_vec()) {
        Err(err) => {
            error!(err = %err, "failed to parse build job name");

            return Arc::new(String::from_utf8_lossy(drv_path).to_string());
        }
        Ok(str) => str,
    };

    let Some(file_stem) = Path::new(&string).file_stem() else {
        error!("drv path build job's file_stem was None");

        return Arc::new(String::from_utf8_lossy(drv_path).to_string());
    };

    let Some(file_stem) = file_stem.to_str() else {
        error!("drv path build job's file_stem was not valid unicode");

        return Arc::new(String::from_utf8_lossy(drv_path).to_string());
    };

    let build_name = file_stem.split_once('-').map_or_else(
        || {
            error!("unexpected drv build job file stem format");
            file_stem
        },
        |(_, name)| name,
    );

    Arc::new(build_name.to_string())
}
