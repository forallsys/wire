// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

#![allow(unused_assignments)]

use std::{num::ParseIntError, path::PathBuf, process::ExitStatus, sync::mpsc::RecvError};

use miette::{Diagnostic, SourceSpan};
use nix_compat::flakeref::{FlakeRef, FlakeRefError};
use thiserror::Error;
use tokio::task::JoinError;

use crate::hive::node::{Name, SwitchToConfigurationGoal};

#[cfg(debug_assertions)]
const DOCS_URL: &str = "http://localhost:5173/reference/errors.html";
#[cfg(not(debug_assertions))]
const DOCS_URL: &str = "https://wire.althaea.zone/reference/errors.html";

#[derive(Debug, Diagnostic, Error)]
pub enum KeyError {
    #[diagnostic(
        code(wire::key::File),
        url("{DOCS_URL}#{}", self.code().unwrap())
    )]
    #[error("error reading file")]
    File(#[source] std::io::Error),

    #[diagnostic(
        code(wire::key::SpawningCommand),
        help("Ensure wire has the correct $PATH for this command"),
        url("{DOCS_URL}#{}", self.code().unwrap())
    )]
    #[error("error spawning key command")]
    CommandSpawnError {
        #[source]
        error: std::io::Error,

        #[source_code]
        command: String,

        #[label(primary, "Program ran")]
        command_span: Option<SourceSpan>,
    },

    #[diagnostic(
        code(wire::key::Resolving),
        url("{DOCS_URL}#{}", self.code().unwrap())
    )]
    #[error("Error resolving key command child process")]
    CommandResolveError {
        #[source]
        error: std::io::Error,

        #[source_code]
        command: String,
    },

    #[diagnostic(
        code(wire::key::CommandExit),
        url("{DOCS_URL}#{}", self.code().unwrap())
    )]
    #[error("key command failed with status {}: {}", .0,.1)]
    CommandError(ExitStatus, String),

    #[diagnostic(
        code(wire::key::Empty),
        url("{DOCS_URL}#{}", self.code().unwrap())
    )]
    #[error("Command list empty")]
    Empty,

    #[diagnostic(
        code(wire::key::ParseKeyPermissions),
        help("Refer to the documentation for the format of key file permissions."),
        url("{DOCS_URL}#{}", self.code().unwrap())
    )]
    #[error("Failed to parse key permissions")]
    ParseKeyPermissions(#[source] ParseIntError),
}

#[derive(Debug, Diagnostic, Error)]
pub enum ActivationError {
    #[diagnostic(
        code(wire::activation::SwitchToConfiguration),
        url("{DOCS_URL}#{}", self.code().unwrap())
    )]
    #[error("failed to run switch-to-configuration {0} on node {1}")]
    SwitchToConfigurationError(SwitchToConfigurationGoal, Name, #[source] CommandError),
}

#[derive(Debug, Diagnostic, Error)]
pub enum NetworkError {
    #[diagnostic(
        code(wire::network::HostUnreachable),
        help(
            "If you failed due to a fault in DNS, note that a node can have multiple targets defined."
        ),
        url("{DOCS_URL}#{}", self.code().unwrap())
    )]
    #[error("Cannot reach host {host}")]
    HostUnreachable {
        host: String,
        #[source]
        source: CommandError,
    },

    #[diagnostic(
        code(wire::network::HostUnreachableAfterReboot),
        url("{DOCS_URL}#{}", self.code().unwrap())
    )]
    #[error("Failed to get regain connection to {0} after activation.")]
    HostUnreachableAfterReboot(String),

    #[diagnostic(
        code(wire::network::HostsExhausted),
        url("{DOCS_URL}#{}", self.code().unwrap())
    )]
    #[error("Ran out of contactable hosts")]
    HostsExhausted,
}

#[derive(Debug, Diagnostic, Error)]
pub enum HiveInitialisationError {
    #[diagnostic(
        code(wire::hive_init::NoHiveFound),
        help(
            "Double check the path is correct. You can adjust the hive path with `--path` when the hive lies outside of the CWD."
        ),
        url("{DOCS_URL}#{}", self.code().unwrap())
    )]
    #[error("No hive could be found in {}", .0.display())]
    NoHiveFound(PathBuf),

    #[diagnostic(
        code(wire::hive_init::Parse),
        help("If you cannot resolve this problem, please create an issue."),
        url("{DOCS_URL}#{}", self.code().unwrap())
    )]
    #[error("Failed to parse internal wire json.")]
    ParseEvaluateError(#[source] serde_json::Error),

    #[diagnostic(
        code(wire::hive_init::ParsePrefetch),
        help("please create an issue."),
        url("{DOCS_URL}#{}", self.code().unwrap())
    )]
    #[error("Failed to parse `nix flake prefetch --json`.")]
    ParsePrefetchError(#[source] serde_json::Error),

    #[diagnostic(
        code(wire::hive_init::NodeDoesNotExist),
        help("Please create an issue!"),
        url("{DOCS_URL}#{}", self.code().unwrap())
    )]
    #[error("node {0} not exist in hive")]
    NodeDoesNotExist(String),
}

#[derive(Debug, Diagnostic, Error)]
pub enum HiveLocationError {
    #[diagnostic(
        code(wire::hive_location::MalformedPath),
        url("{DOCS_URL}#{}", self.code().unwrap())
    )]
    #[error("Path was malformed: {}", .0.display())]
    MalformedPath(PathBuf),

    #[diagnostic(
        code(wire::hive_location::Malformed),
        url("{DOCS_URL}#{}", self.code().unwrap())
    )]
    #[error("--path was malformed")]
    Malformed(#[source] FlakeRefError),

    #[diagnostic(
        code(wire::hive_location::TypeUnsupported),
        url("{DOCS_URL}#{}", self.code().unwrap())
    )]
    #[error("The flakref had an unsupported type: {:#?}", .0)]
    TypeUnsupported(Box<FlakeRef>),
}

#[derive(Debug, Diagnostic, Error)]
pub enum CommandError {
    #[diagnostic(
        code(wire::command::TermAttrs),
        url("{DOCS_URL}#{}", self.code().unwrap())
    )]
    #[error("Failed to set PTY attrs")]
    TermAttrs(#[source] nix::errno::Errno),

    #[diagnostic(
        code(wire::command::PosixPipe),
        url("{DOCS_URL}#{}", self.code().unwrap())
    )]
    #[error("There was an error in regards to a pipe")]
    PosixPipe(#[source] nix::errno::Errno),

    /// Error wrapped around `portable_pty`'s anyhow
    /// errors
    #[diagnostic(
        code(wire::command::PortablePty),
        url("{DOCS_URL}#{}", self.code().unwrap())
    )]
    #[error("There was an error from the portable_pty crate")]
    PortablePty(#[source] anyhow::Error),

    #[diagnostic(
        code(wire::command::Joining),
        url("{DOCS_URL}#{}", self.code().unwrap())
    )]
    #[error("Failed to join on some tokio task")]
    JoinError(#[source] JoinError),

    #[diagnostic(
        code(wire::command::WaitForStatus),
        url("{DOCS_URL}#{}", self.code().unwrap())
    )]
    #[error("Failed to wait for the child's status")]
    WaitForStatus(#[source] std::io::Error),

    #[diagnostic(
        code(wire::detached::NoHandle),
        help("This should never happen, please create an issue!"),
        url("{DOCS_URL}#{}", self.code().unwrap())
    )]
    #[error("There was no handle to child io")]
    NoHandle,

    #[diagnostic(
        code(wire::command::WritingClientStdout),
        url("{DOCS_URL}#{}", self.code().unwrap())
    )]
    #[error("Failed to write to client stderr.")]
    WritingClientStderr(#[source] std::io::Error),

    #[diagnostic(
        code(wire::command::WritingMasterStdin),
        url("{DOCS_URL}#{}", self.code().unwrap())
    )]
    #[error("Failed to write to PTY master stdout.")]
    WritingMasterStdout(#[source] std::io::Error),

    #[diagnostic(
        code(wire::command::Recv),
        url("{DOCS_URL}#{}", self.code().unwrap()),
        help("please create an issue!"),
    )]
    #[error("Failed to receive a message from the begin channel")]
    RecvError(#[source] RecvError),

    #[diagnostic(
        code(wire::command::ThreadPanic),
        url("{DOCS_URL}#{}", self.code().unwrap()),
        help("please create an issue!"),
    )]
    #[error("Thread panicked")]
    ThreadPanic,

    #[diagnostic(
        code(wire::command::CommandFailed),
        url("{DOCS_URL}#{}", self.code().unwrap()),
        help("`nix` commands are filtered, run with -vvv to view all"),
    )]
    #[error("{command_ran} failed ({reason}) with {code} (last 20 lines):\n{logs}")]
    CommandFailed {
        command_ran: String,
        logs: String,
        code: String,
        reason: &'static str,
    },

    #[diagnostic(
        code(wire::command::RuntimeDirectory),
        url("{DOCS_URL}#{}", self.code().unwrap())
    )]
    #[error("error creating $XDG_RUNTIME_DIR/wire")]
    RuntimeDirectory(#[source] std::io::Error),

    #[diagnostic(
        code(wire::command::RuntimeDirectoryMissing),
        url("{DOCS_URL}#{}", self.code().unwrap())
    )]
    #[error("$XDG_RUNTIME_DIR could not be used.")]
    RuntimeDirectoryMissing(#[source] std::env::VarError),

    #[diagnostic(
        code(wire::command::OneshotRecvError),
        url("{DOCS_URL}#{}", self.code().unwrap())
    )]
    #[error("Error waiting for begin message")]
    OneshotRecvError(#[source] tokio::sync::oneshot::error::RecvError),
}

#[derive(Debug, Diagnostic, Error)]
pub enum HiveLibError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    HiveInitialisationError(HiveInitialisationError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    NetworkError(NetworkError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    ActivationError(ActivationError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    CommandError(CommandError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    HiveLocationError(HiveLocationError),

    #[error("Failed to apply key {}", .0)]
    KeyError(
        String,
        #[source]
        #[diagnostic_source]
        KeyError,
    ),

    #[diagnostic(
        code(wire::BuildNode),
        url("{DOCS_URL}#{}", self.code().unwrap())
    )]
    #[error("failed to build node {name}")]
    NixBuildError {
        name: Name,
        #[source]
        source: CommandError,
    },

    #[diagnostic(
        code(wire::CopyPath),
        url("{DOCS_URL}#{}", self.code().unwrap())
    )]
    #[error("failed to copy path {path} to node {name}")]
    NixCopyError {
        name: Name,
        path: String,
        #[source]
        error: Box<CommandError>,
        #[help]
        help: Option<Box<String>>,
    },

    #[diagnostic(code(wire::Evaluate))]
    #[error("failed to evaluate `{attribute}` from the context of a hive.")]
    NixEvalError {
        attribute: String,

        #[source]
        source: CommandError,

        #[help]
        help: Option<Box<String>>,
    },

    #[diagnostic(
        code(wire::Encoding),
        url("{DOCS_URL}#{}", self.code().unwrap())
    )]
    #[error("error encoding length delimited data")]
    Encoding(#[source] std::io::Error),

    #[diagnostic(
        code(wire::SIGINT),
        url("{DOCS_URL}#{}", self.code().unwrap())
    )]
    #[error("SIGINT received, shut down")]
    Sigint,
}
