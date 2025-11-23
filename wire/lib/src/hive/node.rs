// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

#![allow(clippy::missing_errors_doc)]
use enum_dispatch::enum_dispatch;
use gethostname::gethostname;
use serde::{Deserialize, Serialize};
use std::assert_matches::debug_assert_matches;
use std::fmt::Display;
use std::sync::Arc;
use tokio::sync::oneshot;
use tracing::{Instrument, Level, Span, debug, error, event, instrument, trace};

use crate::commands::common::evaluate_hive_attribute;
use crate::commands::{CommandArguments, WireCommandChip, run_command};
use crate::errors::NetworkError;
use crate::hive::HiveLocation;
use crate::hive::steps::build::Build;
use crate::hive::steps::cleanup::CleanUp;
use crate::hive::steps::evaluate::Evaluate;
use crate::hive::steps::keys::{Key, Keys, PushKeyAgent, UploadKeyAt};
use crate::hive::steps::ping::Ping;
use crate::hive::steps::push::{PushBuildOutput, PushEvaluatedOutput};
use crate::status::STATUS;
use crate::{EvalGoal, StrictHostKeyChecking, SubCommandModifiers};

use super::HiveLibError;
use super::steps::activate::SwitchToConfiguration;

#[derive(Serialize, Deserialize, Clone, Debug, Hash, Eq, PartialEq, derive_more::Display)]
pub struct Name(pub Arc<str>);

#[derive(Serialize, Deserialize, Clone, Debug, Hash, Eq, PartialEq)]
pub struct Target {
    pub hosts: Vec<Arc<str>>,
    pub user: Arc<str>,
    pub port: u32,

    #[serde(skip)]
    current_host: usize,
}

impl Target {
    #[instrument(ret(level = tracing::Level::DEBUG), skip_all)]
    pub fn create_ssh_opts(
        &self,
        modifiers: SubCommandModifiers,
        master: bool,
    ) -> Result<String, HiveLibError> {
        self.create_ssh_args(modifiers, false, master)
            .map(|x| x.join(" "))
    }

    #[instrument(ret(level = tracing::Level::DEBUG))]
    pub fn create_ssh_args(
        &self,
        modifiers: SubCommandModifiers,
        non_interactive_forced: bool,
        master: bool,
    ) -> Result<Vec<String>, HiveLibError> {
        let mut vector = vec![
            "-l".to_string(),
            self.user.to_string(),
            "-p".to_string(),
            self.port.to_string(),
        ];
        let mut options = vec![
            format!(
                "StrictHostKeyChecking={}",
                match modifiers.ssh_accept_host {
                    StrictHostKeyChecking::AcceptNew => "accept-new",
                    StrictHostKeyChecking::No => "no",
                }
            )
            .to_string(),
        ];

        options.extend(["PasswordAuthentication=no".to_string()]);
        options.extend(["KbdInteractiveAuthentication=no".to_string()]);

        vector.push("-o".to_string());
        vector.extend(options.into_iter().intersperse("-o".to_string()));

        Ok(vector)
    }
}

#[cfg(test)]
impl Default for Target {
    fn default() -> Self {
        Target {
            hosts: vec!["NAME".into()],
            user: "root".into(),
            port: 22,
            current_host: 0,
        }
    }
}

#[cfg(test)]
impl<'a> Context<'a> {
    fn create_test_context(
        hive_location: HiveLocation,
        name: &'a Name,
        node: &'a mut Node,
    ) -> Self {
        Context {
            name,
            node,
            hive_location: Arc::new(hive_location),
            modifiers: SubCommandModifiers::default(),
            no_keys: false,
            state: StepState::default(),
            goal: Goal::SwitchToConfiguration(SwitchToConfigurationGoal::Switch),
            reboot: false,
            should_apply_locally: false,
            handle_unreachable: HandleUnreachable::default(),
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
        Target {
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
    pub keys: im::Vector<Key>,

    #[serde(rename(deserialize = "_hostPlatform", serialize = "host_platform"))]
    pub host_platform: Arc<str>,

    #[serde(rename(
        deserialize = "privilegeEscalationCommand",
        serialize = "privilege_escalation_command"
    ))]
    pub privilege_escalation_command: im::Vector<Arc<str>>,
}

#[cfg(test)]
impl Default for Node {
    fn default() -> Self {
        Node {
            target: Target::default(),
            keys: im::Vector::new(),
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
        Node {
            target: Target::from_host(host),
            ..Default::default()
        }
    }

    /// Tests the connection to a node
    pub async fn ping(&self, modifiers: SubCommandModifiers) -> Result<(), HiveLibError> {
        let host = self.target.get_preferred_host()?;

        let command_string = format!(
            "ssh {}@{host} {} exit",
            self.target.user,
            self.target.create_ssh_opts(modifiers, true)?
        );

        let output = run_command(
            &CommandArguments::new(command_string, modifiers)
                .log_stdout()
                .mode(crate::commands::ChildOutputMode::Interactive),
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

#[must_use]
pub fn should_apply_locally(allow_local_deployment: bool, name: &str) -> bool {
    *name == *gethostname() && allow_local_deployment
}

#[derive(derive_more::Display)]
pub enum Push<'a> {
    Derivation(&'a Derivation),
    Path(&'a String),
}

#[derive(Deserialize, Clone, Debug)]
pub struct Derivation(String);

impl Display for Derivation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f).and_then(|()| write!(f, "^*"))
    }
}

#[derive(derive_more::Display, Debug, Clone, Copy)]
pub enum SwitchToConfigurationGoal {
    Switch,
    Boot,
    Test,
    DryActivate,
}

#[derive(derive_more::Display, Clone, Copy)]
pub enum Goal {
    SwitchToConfiguration(SwitchToConfigurationGoal),
    Build,
    Push,
    Keys,
}

#[enum_dispatch]
pub(crate) trait ExecuteStep: Send + Sync + Display + std::fmt::Debug {
    async fn execute(&self, ctx: &mut Context<'_>) -> Result<(), HiveLibError>;

    fn should_execute(&self, context: &Context) -> bool;
}

// may include other options such as FailAll in the future
#[non_exhaustive]
#[derive(Clone, Default)]
pub enum HandleUnreachable {
    Ignore,
    #[default]
    FailNode,
}

#[derive(Default)]
pub struct StepState {
    pub evaluation: Option<Derivation>,
    pub evaluation_rx: Option<oneshot::Receiver<Result<Derivation, HiveLibError>>>,
    pub build: Option<String>,
    pub key_agent_directory: Option<String>,
}

pub struct Context<'a> {
    pub name: &'a Name,
    pub node: &'a mut Node,
    pub hive_location: Arc<HiveLocation>,
    pub modifiers: SubCommandModifiers,
    pub no_keys: bool,
    pub state: StepState,
    pub goal: Goal,
    pub reboot: bool,
    pub should_apply_locally: bool,
    pub handle_unreachable: HandleUnreachable,
}

#[enum_dispatch(ExecuteStep)]
#[derive(Debug, PartialEq)]
enum Step {
    Ping,
    PushKeyAgent,
    Keys,
    Evaluate,
    PushEvaluatedOutput,
    Build,
    PushBuildOutput,
    SwitchToConfiguration,
    CleanUp,
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
            Self::CleanUp(step) => step.fmt(f),
        }
    }
}

pub struct GoalExecutor<'a> {
    steps: Vec<Step>,
    context: Context<'a>,
}

impl<'a> GoalExecutor<'a> {
    #[must_use]
    pub fn new(context: Context<'a>) -> Self {
        Self {
            steps: vec![
                Step::Ping(Ping),
                Step::PushKeyAgent(PushKeyAgent),
                Step::Keys(Keys {
                    filter: UploadKeyAt::NoFilter,
                }),
                Step::Keys(Keys {
                    filter: UploadKeyAt::PreActivation,
                }),
                Step::Evaluate(super::steps::evaluate::Evaluate),
                Step::PushEvaluatedOutput(super::steps::push::PushEvaluatedOutput),
                Step::Build(super::steps::build::Build),
                Step::PushBuildOutput(super::steps::push::PushBuildOutput),
                Step::SwitchToConfiguration(SwitchToConfiguration),
                Step::Keys(Keys {
                    filter: UploadKeyAt::PostActivation,
                }),
                Step::CleanUp(CleanUp),
            ],
            context,
        }
    }

    #[instrument(skip_all, name = "eval")]
    async fn evaluate_task(
        tx: oneshot::Sender<Result<Derivation, HiveLibError>>,
        hive_location: Arc<HiveLocation>,
        name: Name,
        modifiers: SubCommandModifiers,
    ) {
        let output =
            evaluate_hive_attribute(&hive_location, &EvalGoal::GetTopLevel(&name), modifiers)
                .await
                .map(|output| {
                    serde_json::from_str::<Derivation>(&output).expect("failed to parse derivation")
                });

        debug!(output = ?output, done = true);

        let _ = tx.send(output);
    }

    #[instrument(skip_all, fields(node = %self.context.name))]
    pub async fn execute(mut self) -> Result<(), HiveLibError> {
        let (tx, rx) = oneshot::channel();
        self.context.state.evaluation_rx = Some(rx);

        // The name of this span should never be changed without updating
        // `wire/cli/tracing_setup.rs`
        debug_assert_matches!(Span::current().metadata().unwrap().name(), "execute");
        // This span should always have a `node` field by the same file
        debug_assert!(
            Span::current()
                .metadata()
                .unwrap()
                .fields()
                .field("node")
                .is_some()
        );

        if !matches!(self.context.goal, Goal::Keys) {
            tokio::spawn(
                GoalExecutor::evaluate_task(
                    tx,
                    self.context.hive_location.clone(),
                    self.context.name.clone(),
                    self.context.modifiers,
                )
                .in_current_span(),
            );
        }

        let steps = self
            .steps
            .iter()
            .filter(|step| step.should_execute(&self.context))
            .inspect(|step| {
                trace!("Will execute step `{step}` for {}", self.context.name);
            })
            .collect::<Vec<_>>();
        let length = steps.len();

        for (position, step) in steps.iter().enumerate() {
            event!(
                Level::INFO,
                step = step.to_string(),
                progress = format!("{}/{length}", position + 1)
            );

            STATUS
                .lock()
                .set_node_step(self.context.name, step.to_string());

            if let Err(err) = step.execute(&mut self.context).await.inspect_err(|_| {
                error!("Failed to execute `{step}`");
            }) {
                // discard error from cleanup
                let _ = CleanUp.execute(&mut self.context).await;

                if matches!(step, Step::Ping(..))
                    && matches!(self.context.handle_unreachable, HandleUnreachable::Ignore)
                {
                    return Ok(());
                }

                STATUS.lock().mark_node_failed(self.context.name);

                return Err(err);
            }
        }

        STATUS.lock().mark_node_succeeded(self.context.name);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use rand::distr::Alphabetic;

    use super::*;
    use crate::{
        function_name, get_test_path,
        hive::{Hive, get_hive_location},
        location,
    };
    use std::{assert_matches::assert_matches, path::PathBuf};
    use std::{collections::HashMap, env};

    fn get_steps(goal_executor: GoalExecutor) -> std::vec::Vec<Step> {
        goal_executor
            .steps
            .into_iter()
            .filter(|step| step.should_execute(&goal_executor.context))
            .collect::<Vec<_>>()
    }

    #[tokio::test]
    #[cfg_attr(feature = "no_web_tests", ignore)]
    async fn default_values_match() {
        let mut path = get_test_path!();

        let location = get_hive_location(path.display().to_string()).unwrap();
        let hive = Hive::new_from_path(&location, SubCommandModifiers::default())
            .await
            .unwrap();

        let node = Node::default();

        let mut nodes = HashMap::new();
        nodes.insert(Name("NAME".into()), node);

        path.push("hive.nix");

        assert_eq!(
            hive,
            Hive {
                nodes,
                schema: Hive::SCHEMA_VERSION
            }
        );
    }

    #[tokio::test]
    async fn order_build_locally() {
        let location = location!(get_test_path!());
        let mut node = Node {
            build_remotely: false,
            ..Default::default()
        };
        let name = &Name(function_name!().into());
        let executor = GoalExecutor::new(Context::create_test_context(location, name, &mut node));
        let steps = get_steps(executor);

        assert_eq!(
            steps,
            vec![
                Ping.into(),
                PushKeyAgent.into(),
                Keys {
                    filter: UploadKeyAt::PreActivation
                }
                .into(),
                crate::hive::steps::evaluate::Evaluate.into(),
                crate::hive::steps::build::Build.into(),
                crate::hive::steps::push::PushBuildOutput.into(),
                SwitchToConfiguration.into(),
                Keys {
                    filter: UploadKeyAt::PostActivation
                }
                .into(),
                CleanUp.into()
            ]
        );
    }

    #[tokio::test]
    async fn order_keys_only() {
        let location = location!(get_test_path!());
        let mut node = Node::default();
        let name = &Name(function_name!().into());
        let mut context = Context::create_test_context(location, name, &mut node);

        context.goal = Goal::Keys;

        let executor = GoalExecutor::new(context);
        let steps = get_steps(executor);

        assert_eq!(
            steps,
            vec![
                Ping.into(),
                PushKeyAgent.into(),
                Keys {
                    filter: UploadKeyAt::NoFilter
                }
                .into(),
                CleanUp.into()
            ]
        );
    }

    #[tokio::test]
    async fn order_build_only() {
        let location = location!(get_test_path!());
        let mut node = Node::default();
        let name = &Name(function_name!().into());
        let mut context = Context::create_test_context(location, name, &mut node);

        context.goal = Goal::Build;

        let executor = GoalExecutor::new(context);
        let steps = get_steps(executor);

        assert_eq!(
            steps,
            vec![
                Ping.into(),
                crate::hive::steps::evaluate::Evaluate.into(),
                crate::hive::steps::build::Build.into(),
                crate::hive::steps::push::PushBuildOutput.into(),
                CleanUp.into()
            ]
        );
    }

    #[tokio::test]
    async fn order_push_only() {
        let location = location!(get_test_path!());
        let mut node = Node::default();
        let name = &Name(function_name!().into());
        let mut context = Context::create_test_context(location, name, &mut node);

        context.goal = Goal::Push;

        let executor = GoalExecutor::new(context);
        let steps = get_steps(executor);

        assert_eq!(
            steps,
            vec![
                Ping.into(),
                crate::hive::steps::evaluate::Evaluate.into(),
                crate::hive::steps::push::PushEvaluatedOutput.into(),
                CleanUp.into()
            ]
        );
    }

    #[tokio::test]
    async fn order_remote_build() {
        let location = location!(get_test_path!());
        let mut node = Node {
            build_remotely: true,
            ..Default::default()
        };

        let name = &Name(function_name!().into());
        let executor = GoalExecutor::new(Context::create_test_context(location, name, &mut node));
        let steps = get_steps(executor);

        assert_eq!(
            steps,
            vec![
                Ping.into(),
                PushKeyAgent.into(),
                Keys {
                    filter: UploadKeyAt::PreActivation
                }
                .into(),
                crate::hive::steps::evaluate::Evaluate.into(),
                crate::hive::steps::push::PushEvaluatedOutput.into(),
                crate::hive::steps::build::Build.into(),
                SwitchToConfiguration.into(),
                Keys {
                    filter: UploadKeyAt::PostActivation
                }
                .into(),
                CleanUp.into()
            ]
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
            "PasswordAuthentication=no".to_string(),
            "-o".to_string(),
            "KbdInteractiveAuthentication=no".to_string(),
        ];

        assert_eq!(
            target
                .create_ssh_args(subcommand_modifiers, false, false)
                .unwrap(),
            args
        );
        assert_eq!(
            target.create_ssh_opts(subcommand_modifiers, false).unwrap(),
            args.join(" ")
        );

        assert_eq!(
            target
                .create_ssh_args(subcommand_modifiers, false, true)
                .unwrap(),
            [
                "-l".to_string(),
                target.user.to_string(),
                "-p".to_string(),
                target.port.to_string(),
                "-o".to_string(),
                "StrictHostKeyChecking=accept-new".to_string(),
                "-o".to_string(),
                "PasswordAuthentication=no".to_string(),
                "-o".to_string(),
                "KbdInteractiveAuthentication=no".to_string(),
            ]
        );

        assert_eq!(
            target
                .create_ssh_args(subcommand_modifiers, true, true)
                .unwrap(),
            [
                "-l".to_string(),
                target.user.to_string(),
                "-p".to_string(),
                target.port.to_string(),
                "-o".to_string(),
                "StrictHostKeyChecking=accept-new".to_string(),
                "-o".to_string(),
                "PasswordAuthentication=no".to_string(),
                "-o".to_string(),
                "KbdInteractiveAuthentication=no".to_string(),
            ]
        );

        // forced non interactive is the same as --non-interactive
        assert_eq!(
            target
                .create_ssh_args(subcommand_modifiers, true, false)
                .unwrap(),
            target
                .create_ssh_args(
                    SubCommandModifiers {
                        non_interactive: true,
                        ..Default::default()
                    },
                    false,
                    false
                )
                .unwrap()
        );
    }
}
