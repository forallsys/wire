// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

use crate::commands::pty::{InteractiveChildChip, interactive_command_with_env};
use std::{collections::HashMap, hash::BuildHasher, str::from_utf8, sync::{Arc, LazyLock}};

use aho_corasick::AhoCorasick;
use gjson::Value;
use itertools::Itertools;
use nix_compat::log::{AT_NIX_PREFIX, VerbosityLevel};
use num_enum::TryFromPrimitive;
use tracing::{debug, error, info, trace, warn};

use crate::{
    SubCommandModifiers,
    commands::noninteractive::{NonInteractiveChildChip, non_interactive_command_with_env},
    errors::{CommandError, HiveLibError},
    hive::node::{Node, Target},
};

pub mod builder;
pub mod common;
pub(crate) mod noninteractive;
pub(crate) mod pty;

#[derive(Copy, Clone, Debug)]
pub enum ChildOutputMode {
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
pub struct CommandArguments<'t, S: AsRef<str>> {
    modifiers: SubCommandModifiers,
    target: Option<&'t Target>,
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

impl<'a, S: AsRef<str>> CommandArguments<'a, S> {
    pub const fn new(command_string: S, modifiers: SubCommandModifiers) -> Self {
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

    #[must_use]
    pub const fn execute_on_remote(mut self, target: Option<&'a Target>) -> Self {
        self.target = target;
        self
    }

    #[must_use]
    pub const fn mode(mut self, mode: ChildOutputMode) -> Self {
        self.output_mode = mode;
        self
    }

    #[must_use]
    pub const fn keep_stdin_open(mut self) -> Self {
        self.keep_stdin_open = true;
        self
    }

    #[must_use]
    pub fn privileged(mut self, escalation_command: &[Arc<str>]) -> Self {
        self.privilege_escalation_command =
            Some(escalation_command.iter().join(" "));
        self
    }

    #[must_use]
    pub const fn is_elevated(&self) -> bool {
        self.privilege_escalation_command.is_some()
    }

    #[must_use]
    pub const fn log_stdout(mut self) -> Self {
        self.log_stdout = true;
        self
    }
}

pub async fn run_command<S: AsRef<str>>(
    arguments: &CommandArguments<'_, S>,
) -> Result<Either<InteractiveChildChip, NonInteractiveChildChip>, HiveLibError> {
    run_command_with_env(arguments, HashMap::new()).await
}

pub async fn run_command_with_env<S: AsRef<str>, B: BuildHasher>(
    arguments: &CommandArguments<'_, S>,
    envs: HashMap<String, String, B>,
) -> Result<Either<InteractiveChildChip, NonInteractiveChildChip>, HiveLibError> {
    // use the non interactive command runner when forced
    // ... or when there is no reason for interactivity, local and unprivileged
    if arguments.modifiers.non_interactive
        || (arguments.target.is_none() && !arguments.is_elevated())
    {
        return Ok(Either::Right(non_interactive_command_with_env(
            arguments, envs,
        )?));
    }

    Ok(Either::Left(
        interactive_command_with_env(arguments, envs).await?,
    ))
}

pub trait WireCommandChip {
    type ExitStatus;
    
    #[allow(async_fn_in_trait)]
    async fn wait_till_success(self) -> Result<Self::ExitStatus, CommandError>;
    #[allow(async_fn_in_trait)]
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

fn trace_gjson_str<'a>(log: &'a Value<'a>, msg: &'a str) -> Option<String> {
    if msg.is_empty() {
        return None;
    }

    let level = log.get("level");

    if !level.exists() {
        return None;
    }

    let level = match VerbosityLevel::try_from_primitive(level.u64()) {
        Ok(level) => level,
        Err(err) => {
            error!("nix log `level` did not match to a VerbosityLevel: {err:?}");
            return None;
        }
    };

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

impl ChildOutputMode {
    /// this function is by far the biggest hotspot in the whole tree
    /// Returns a string if this log is notable to be stored as an error message
    fn trace_slice(self, line: &mut [u8]) -> Option<String> {
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

        let Ok(str) = from_utf8(slice) else {
            error!("nix log was not valid utf8!");
            return None;
        };

        let log = gjson::parse(str);

        let text = log.get("text");

        if text.exists() {
            return trace_gjson_str(&log, text.str());
        }

        let text = log.get("msg");

        if text.exists() {
            return trace_gjson_str(&log, text.str());
        }

        None
    }
}
