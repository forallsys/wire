// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

#![allow(unused_assignments)]

use std::{num::ParseIntError, path::PathBuf, process::ExitStatus, sync::mpsc::RecvError};

use miette::{Diagnostic, SourceSpan};
use nix_compat::flakeref::{FlakeRef, FlakeRefError};
use thiserror::Error;
use tokio::task::JoinError;
use wire_nix_client::{NixDaemonClientError, store_path::StorePathError};

use crate::{
    SafeStorePath,
    hive::node::{Name, SwitchToConfigurationGoal},
};

#[derive(Debug, Diagnostic, Error)]
pub enum KeyError {
    #[diagnostic(code(wire::key::File))]
    #[error("error reading file")]
    File(#[source] std::io::Error),

    #[diagnostic(
        code(wire::key::SpawningCommand),
        help("Ensure wire has the correct $PATH for this command")
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

    #[diagnostic(code(wire::key::Resolving))]
    #[error("Error resolving key command child process")]
    CommandResolveError {
        #[source]
        error: std::io::Error,

        #[source_code]
        command: String,
    },

    #[diagnostic(code(wire::key::CommandExit))]
    #[error("key command failed with status {}: {}", .0,.1)]
    CommandError(ExitStatus, String),

    #[diagnostic(code(wire::key::Empty))]
    #[error("Command list empty")]
    Empty,

    #[diagnostic(
        code(wire::key::ParseKeyPermissions),
        help("Refer to the documentation for the format of key file permissions.")
    )]
    #[error("Failed to parse key permissions")]
    ParseKeyPermissions(#[source] ParseIntError),
}

#[derive(Debug, Diagnostic, Error)]
pub enum ActivationError {
    #[diagnostic(code(wire::activation::SwitchToConfiguration))]
    #[error("failed to run switch-to-configuration {0} on node {1}")]
    SwitchToConfigurationError(SwitchToConfigurationGoal, Name, #[source] CommandError),
}

#[derive(Debug, Diagnostic, Error)]
pub enum NetworkError {
    #[diagnostic(
        code(wire::network::HostUnreachable),
        help(
            "If you failed due to a fault in DNS, note that a node can have multiple targets defined."
        )
    )]
    #[error("Cannot reach host {host}")]
    HostUnreachable {
        host: String,
        #[source]
        source: CommandError,
    },

    #[diagnostic(code(wire::network::HostUnreachableAfterReboot))]
    #[error("Failed to get regain connection to {0} after activation.")]
    HostUnreachableAfterReboot(String),

    #[diagnostic(code(wire::network::HostsExhausted))]
    #[error("Ran out of contactable hosts")]
    HostsExhausted,
}

#[derive(Debug, Diagnostic, Error)]
pub enum HiveInitialisationError {
    #[diagnostic(
        code(wire::hive_init::NoHiveFound),
        help(
            "Double check the path is correct. You can adjust the hive path with `--path` when the hive lies outside of the CWD."
        )
    )]
    #[error("No hive could be found in {}", .0.display())]
    NoHiveFound(PathBuf),

    #[diagnostic(
        code(wire::hive_init::Parse),
        help("If you cannot resolve this problem, please create an issue.")
    )]
    #[error("Failed to parse internal wire json.")]
    ParseEvaluateError(#[source] serde_json::Error),

    #[diagnostic(code(wire::hive_init::ParsePrefetch), help("please create an issue."))]
    #[error("Failed to parse `nix flake prefetch --json`.")]
    ParsePrefetchError(#[source] serde_json::Error),

    #[diagnostic(
        code(wire::hive_init::NodeDoesNotExist),
        help("Please create an issue!")
    )]
    #[error("node {0} not exist in hive")]
    NodeDoesNotExist(String),
}

#[derive(Debug, Diagnostic, Error)]
pub enum HiveLocationError {
    #[diagnostic(code(wire::hive_location::MalformedPath))]
    #[error("Path was malformed: {}", .0.display())]
    MalformedPath(PathBuf),

    #[diagnostic(code(wire::hive_location::Malformed))]
    #[error("--path was malformed")]
    Malformed(#[source] FlakeRefError),

    #[diagnostic(code(wire::hive_location::TypeUnsupported))]
    #[error("The flakref had an unsupported type: {:#?}", .0)]
    TypeUnsupported(Box<FlakeRef>),
}

#[derive(Debug, Diagnostic, Error)]
pub enum CommandError {
    #[diagnostic(code(wire::command::TermAttrs))]
    #[error("Failed to set PTY attrs")]
    TermAttrs(#[source] nix::errno::Errno),

    #[diagnostic(code(wire::command::PosixPipe))]
    #[error("There was an error in regards to a pipe")]
    PosixPipe(#[source] nix::errno::Errno),

    /// Error wrapped around `portable_pty`'s anyhow
    /// errors
    #[diagnostic(code(wire::command::PortablePty))]
    #[error("There was an error from the portable_pty crate")]
    PortablePty(#[source] anyhow::Error),

    #[diagnostic(code(wire::command::Joining))]
    #[error("Failed to join on some tokio task")]
    JoinError(#[source] JoinError),

    #[diagnostic(code(wire::command::WaitForStatus))]
    #[error("Failed to wait for the child's status")]
    WaitForStatus(#[source] std::io::Error),

    #[diagnostic(
        code(wire::detached::NoHandle),
        help("This should never happen, please create an issue!")
    )]
    #[error("There was no handle to child io")]
    NoHandle,

    #[diagnostic(code(wire::command::WritingClientStdout))]
    #[error("Failed to write to client stderr.")]
    WritingClientStderr(#[source] std::io::Error),

    #[diagnostic(code(wire::command::WritingMasterStdin))]
    #[error("Failed to write to PTY master stdout.")]
    WritingMasterStdout(#[source] std::io::Error),

    #[diagnostic(code(wire::command::Recv), help("please create an issue!"))]
    #[error("Failed to receive a message from the begin channel")]
    RecvError(#[source] RecvError),

    #[diagnostic(code(wire::command::ThreadPanic), help("please create an issue!"))]
    #[error("Thread panicked")]
    ThreadPanic,

    #[diagnostic(
        code(wire::command::CommandFailed),
        help("`nix` commands are filtered, run with -vvv to view all")
    )]
    #[error("{command_ran} failed ({reason}) with {code} (last 20 lines):\n{logs}")]
    CommandFailed {
        command_ran: String,
        logs: String,
        code: String,
        reason: &'static str,
    },

    #[diagnostic(code(wire::command::RuntimeDirectory))]
    #[error("error creating $XDG_RUNTIME_DIR/wire")]
    RuntimeDirectory(#[source] std::io::Error),

    #[diagnostic(code(wire::command::RuntimeDirectoryMissing))]
    #[error("$XDG_RUNTIME_DIR could not be used.")]
    RuntimeDirectoryMissing(#[source] std::env::VarError),

    #[diagnostic(code(wire::command::OneshotRecvError))]
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

    #[error(transparent)]
    #[diagnostic(transparent)]
    NixDaemonClientError(NixDaemonClientError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    StorePath(StorePathError),

    #[error("Failed to apply key {}", .0)]
    KeyError(
        String,
        #[source]
        #[diagnostic_source]
        KeyError,
    ),

    /// Regular nix build error
    #[diagnostic(code(wire::BuildNodeDaemon))]
    #[error("failed to build node {name}")]
    NixBuildError {
        name: Name,
        #[source]
        source: NixDaemonClientError,
    },

    /// Nix daemon nix build error
    #[diagnostic(code(wire::BuildNodeCli))]
    #[error("failed to build node {name}")]
    NixBuildCliError {
        name: Name,
        #[source]
        source: CommandError,
    },

    /// Regular nix copy error
    #[diagnostic(code(wire::CopyPathDaemon))]
    #[error("failed to copy path {} to node {name}", path.to_absolute_path())]
    NixCopyError {
        name: Name,
        path: SafeStorePath<String>,
        #[source]
        error: Box<NixDaemonClientError>,
        #[help]
        help: Option<String>,
    },

    /// Experimental nix daemon client copy error
    #[diagnostic(code(wire::CopyPathCli))]
    #[error("failed to copy path {} to node {name}", path.to_absolute_path())]
    NixCopyCliError {
        name: Name,
        path: SafeStorePath<String>,
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

    #[diagnostic(code(wire::KeyArchitectureNotFound))]
    #[error("{arg_name} environment variable not set! \n
                wire was not built with the ability to deploy keys to this platform. \n
                Please create an issue: https://github.com/forallsys/wire/issues/new?template=bug_report.md")]
    KeyArchitectureNotFound {
        arg_name: String
    },

    #[diagnostic(code(wire::Encoding))]
    #[error("error encoding length delimited data")]
    Encoding(#[source] std::io::Error),

    #[diagnostic(code(wire::SIGINT))]
    #[error("SIGINT received, shut down")]
    Sigint,
}

impl From<StorePathError> for HiveLibError {
    fn from(e: StorePathError) -> Self {
        HiveLibError::StorePath(e)
    }
}
