// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

#![allow(clippy::missing_errors_doc)]
use enum_dispatch::enum_dispatch;
use gethostname::gethostname;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::Display;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::nonpoison::Mutex;
use tokio::sync::{RwLock, oneshot};
use tracing::instrument;

use crate::commands::builder::CommandStringBuilder;
use crate::commands::{CommandArguments, WireCommandChip, run_command, trace_nix_log_message};
use crate::errors::NetworkError;
use crate::hive::HiveLocation;
use crate::hive::steps::build::Build;
use crate::hive::steps::evaluate::Evaluate;
use crate::hive::steps::keys::{Key, Keys, PushKeyAgent};
use crate::hive::steps::ping::Ping;
use crate::hive::steps::push::{PushBuildOutput, PushEvaluatedOutput};
use crate::{SafeStorePath, StrictHostKeyChecking, SubCommandModifiers, open_remote_client};

use super::HiveLibError;
use super::steps::activate::SwitchToConfiguration;

#[derive(
    Serialize, Deserialize, Clone, Debug, Hash, Eq, PartialEq, PartialOrd, Ord, derive_more::Display,
)]
pub struct Name(pub Arc<str>);

#[derive(Serialize, Deserialize, Clone, Debug, Hash, Eq, PartialEq)]
pub struct Target {
    pub hosts: Vec<Arc<str>>,
    pub user: Arc<str>,
    pub port: u32,

    #[serde(skip)]
    current_host: usize,
}

#[derive(Clone, Debug)]
pub struct SharedTarget(pub Arc<RwLock<Target>>);

// Hack specifically for testing if two steps that have the same shared target
// are equal
#[cfg(test)]
impl PartialEq for SharedTarget {
    fn eq(&self, other: &Self) -> bool {
        let self_guard = self
            .0
            .try_read()
            .expect("failed to target read in test context");
        let other_guard = other
            .0
            .try_read()
            .expect("failed to target read in test context");

        *self_guard == *other_guard
    }
}

impl Target {
    #[instrument(ret(level = tracing::Level::DEBUG), skip_all)]
    pub fn create_ssh_opts(&self, modifiers: SubCommandModifiers) -> Result<String, HiveLibError> {
        self.create_ssh_args(modifiers, false).map(|x| x.join(" "))
    }

    #[instrument(ret(level = tracing::Level::DEBUG))]
    pub fn create_ssh_args(
        &self,
        modifiers: SubCommandModifiers,
        force_quiet: bool,
    ) -> Result<Vec<String>, HiveLibError> {
        let mut vector = vec![
            "-l".to_string(),
            self.user.to_string(),
            "-p".to_string(),
            self.port.to_string(),
        ];
        let mut options = vec![format!(
            "StrictHostKeyChecking={}",
            match modifiers.ssh_accept_host {
                StrictHostKeyChecking::AcceptNew => "accept-new",
                StrictHostKeyChecking::No => "no",
            }
        )];

        options.extend(["BatchMode=yes".to_string()]);

        vector.push("-o".to_string());
        vector.extend(options.into_iter().intersperse("-o".to_string()));

        if force_quiet {
            vector.push("-q".to_string());
        } else if modifiers.ssh_verbosity > 0 {
            vector.push(format!("-{}", "v".repeat(modifiers.ssh_verbosity)));
        }

        Ok(vector)
    }

    /// Tests the connection to a node
    pub async fn ping(
        &self,
        modifiers: SubCommandModifiers,
        should_quit: Arc<AtomicBool>,
    ) -> Result<(), HiveLibError> {
        if modifiers.experimental_nix_client {
            open_remote_client(&self, modifiers, trace_nix_log_message, should_quit).await?;
            return Ok(());
        }

        let host = self.get_preferred_host()?;

        let mut command_string = CommandStringBuilder::new("ssh");
        command_string.arg(format!("{}@{host}", self.user));
        command_string.arg(self.create_ssh_opts(modifiers)?);
        command_string.arg("exit");

        let output = run_command(
            &CommandArguments::new(command_string, modifiers)
                .log_stdout()
                .mode(crate::commands::ChildOutputMode::Generic),
        )
        .await?;

        output.wait_till_success().await.map_err(|source| {
            HiveLibError::NetworkError(NetworkError::HostUnreachable {
                host: host.to_string(),
                source,
            })
        })?;

        Ok(())
    }
}

#[cfg(test)]
impl Default for Target {
    fn default() -> Self {
        Self {
            hosts: vec!["NAME".into()],
            user: "root".into(),
            port: 22,
            current_host: 0,
        }
    }
}

impl Target {
    pub fn get_preferred_host(&self) -> Result<&Arc<str>, HiveLibError> {
        self.hosts
            .get(self.current_host)
            .ok_or(HiveLibError::NetworkError(NetworkError::HostsExhausted))
    }

    pub const fn host_failed(&mut self) {
        self.current_host += 1;
    }

    #[cfg(test)]
    #[must_use]
    pub fn from_host(host: &str) -> Self {
        Self {
            hosts: vec![host.into()],
            ..Default::default()
        }
    }
}

impl Display for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let hosts = itertools::Itertools::join(
            &mut self
                .hosts
                .iter()
                .map(|host| format!("{}@{host}:{}", self.user, self.port)),
            ", ",
        );

        write!(f, "{hosts}")
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Eq, PartialEq, Hash)]
pub struct Node {
    #[serde(rename = "target")]
    pub target: Target,

    #[serde(rename = "buildOnTarget")]
    pub build_remotely: bool,

    #[serde(rename = "allowLocalDeployment")]
    pub allow_local_deployment: bool,

    #[serde(default)]
    pub tags: im::HashSet<String>,

    #[serde(rename(deserialize = "_keys", serialize = "keys"))]
    pub keys: Vec<Arc<Key>>,

    #[serde(rename(deserialize = "_hostPlatform", serialize = "host_platform"))]
    pub host_platform: Arc<str>,

    #[serde(rename(
        deserialize = "privilegeEscalationCommand",
        serialize = "privilege_escalation_command"
    ))]
    pub privilege_escalation_command: Arc<Vec<Arc<str>>>,
}

#[cfg(test)]
impl Default for Node {
    fn default() -> Self {
        Self {
            target: Target::default(),
            keys: Vec::new(),
            tags: im::HashSet::new(),
            privilege_escalation_command: vec!["sudo".into(), "--".into()].into(),
            allow_local_deployment: true,
            build_remotely: false,
            host_platform: "x86_64-linux".into(),
        }
    }
}

impl Node {
    #[cfg(test)]
    #[must_use]
    pub fn from_host(host: &str) -> Self {
        Self {
            target: Target::from_host(host),
            ..Default::default()
        }
    }
}

#[must_use]
pub fn should_apply_locally(allow_local_deployment: bool, name: &str) -> bool {
    *name == *gethostname() && allow_local_deployment
}

#[derive(Debug)]
pub enum Push<'a> {
    Derivation(&'a SafeStorePath<String>),
    Path(&'a SafeStorePath<String>),
}

#[derive(derive_more::Display, Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchToConfigurationGoal {
    Switch,
    Boot,
    Test,
    DryActivate,
}

#[derive(derive_more::Display, Debug, Clone, Copy)]
pub enum ApplyGoal {
    SwitchToConfiguration(SwitchToConfigurationGoal),
    Build,
    Push,
    Keys,
}

#[enum_dispatch]
pub(crate) trait ExecuteStep: Send + Sync + Display + std::fmt::Debug {
    async fn execute(&self, ctx: &mut Context) -> Result<(), HiveLibError>;
}

// may include other options such as FailAll in the future
#[non_exhaustive]
#[derive(Clone, Copy, Default)]
pub enum HandleUnreachable {
    Ignore,
    #[default]
    FailNode,
}

#[derive(Default)]
pub struct StepState {
    pub evaluation_rx: Option<oneshot::Receiver<Result<SafeStorePath<String>, HiveLibError>>>,
}

pub type BuildNameMap = Arc<Mutex<HashMap<u64, Arc<String>>>>;

pub struct Context {
    pub hive_location: Arc<HiveLocation>,
    pub modifiers: SubCommandModifiers,
    pub state: StepState,
    pub should_quit: Arc<AtomicBool>,
    pub name: Name,

    pub build_id_names: BuildNameMap,
}

#[enum_dispatch(ExecuteStep)]
#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq))]
pub enum Step {
    Ping,
    PushKeyAgent,
    Keys,
    Evaluate,
    PushEvaluatedOutput,
    Build,
    PushBuildOutput,
    SwitchToConfiguration,
}

impl Display for Step {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ping(step) => step.fmt(f),
            Self::PushKeyAgent(step) => step.fmt(f),
            Self::Keys(step) => step.fmt(f),
            Self::Evaluate(step) => step.fmt(f),
            Self::PushEvaluatedOutput(step) => step.fmt(f),
            Self::Build(step) => step.fmt(f),
            Self::PushBuildOutput(step) => step.fmt(f),
            Self::SwitchToConfiguration(step) => step.fmt(f),
        }
    }
}

#[cfg(test)]
mod tests {
    use rand::distr::Alphabetic;

    use super::*;
    use std::{assert_matches, env};

    #[test]
    fn test_ssh_opts() {
        let target = Target::from_host("hello-world");
        let subcommand_modifiers = SubCommandModifiers {
            non_interactive: false,
            ..Default::default()
        };
        let tmp = format!(
            "/tmp/{}",
            rand::distr::SampleString::sample_string(&Alphabetic, &mut rand::rng(), 10)
        );

        std::fs::create_dir(&tmp).unwrap();

        unsafe { env::set_var("XDG_RUNTIME_DIR", &tmp) }

        let args = [
            "-l".to_string(),
            target.user.to_string(),
            "-p".to_string(),
            target.port.to_string(),
            "-o".to_string(),
            "StrictHostKeyChecking=accept-new".to_string(),
            "-o".to_string(),
            "BatchMode=yes".to_string(),
        ];

        assert_eq!(
            target.create_ssh_args(subcommand_modifiers, false).unwrap(),
            args
        );
        assert_eq!(
            target.create_ssh_opts(subcommand_modifiers).unwrap(),
            args.join(" ")
        );

        assert_eq!(
            target.create_ssh_args(subcommand_modifiers, false).unwrap(),
            [
                "-l".to_string(),
                target.user.to_string(),
                "-p".to_string(),
                target.port.to_string(),
                "-o".to_string(),
                "StrictHostKeyChecking=accept-new".to_string(),
                "-o".to_string(),
                "BatchMode=yes".to_string(),
            ]
        );

        assert_eq!(
            target.create_ssh_args(subcommand_modifiers, false).unwrap(),
            [
                "-l".to_string(),
                target.user.to_string(),
                "-p".to_string(),
                target.port.to_string(),
                "-o".to_string(),
                "StrictHostKeyChecking=accept-new".to_string(),
                "-o".to_string(),
                "BatchMode=yes".to_string(),
            ]
        );

        // forced non interactive is the same as --non-interactive
        assert_eq!(
            target.create_ssh_args(subcommand_modifiers, false).unwrap(),
            target
                .create_ssh_args(
                    SubCommandModifiers {
                        non_interactive: true,
                        ..Default::default()
                    },
                    false
                )
                .unwrap()
        );
    }

    #[test]
    fn target_fails_increments() {
        let mut target = Target::from_host("localhost");

        assert_eq!(target.current_host, 0);

        for i in 0..100 {
            target.host_failed();
            assert_eq!(target.current_host, i + 1);
        }
    }

    #[test]
    fn get_preferred_host_fails() {
        let mut target = Target {
            hosts: vec![
                "un.reachable.1".into(),
                "un.reachable.2".into(),
                "un.reachable.3".into(),
                "un.reachable.4".into(),
                "un.reachable.5".into(),
            ],
            ..Default::default()
        };

        assert_ne!(
            target.get_preferred_host().unwrap().to_string(),
            "un.reachable.5"
        );

        for i in 1..=5 {
            assert_eq!(
                target.get_preferred_host().unwrap().to_string(),
                format!("un.reachable.{i}")
            );
            target.host_failed();
        }

        for _ in 0..5 {
            assert_matches!(
                target.get_preferred_host(),
                Err(HiveLibError::NetworkError(NetworkError::HostsExhausted))
            );
        }
    }
}
