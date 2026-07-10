use std::{
    collections::HashMap,
    fmt::Display,
    pin::Pin,
    sync::{Arc, atomic::AtomicBool},
};

use futures::{FutureExt, future::Shared};
use itertools::Itertools;
use petgraph::{
    Direction,
    algo::toposort,
    graph::{DiGraph, NodeIndex},
    visit::EdgeRef,
};
use tokio::sync::RwLock;

use crate::{
    SafeStorePath, SubCommandModifiers,
    errors::HiveLibError,
    hive::{
        HiveLocation,
        node::{
            ApplyGoal, Context, ExecuteStep, HandleUnreachable, Name, Node, SharedTarget, Step,
            SwitchToConfigurationGoal,
        },
        steps::{
            activate::SwitchToConfiguration,
            build::{Build, BuildMetadata, NixCommandBuildMetadata},
            evaluate::Evaluate,
            keys::{Keys, PushKeyAgent, UploadKeyAt},
            ping::Ping,
            push::{PushOutput, PushOutputKind},
        },
    },
};

pub struct NodePlan {
    pub context: Context,
    pub steps: Vec<Step>,
    pub greedy_evaluate: bool,
    pub ignore_failed_ping: bool,
}

#[allow(clippy::struct_excessive_bools)]
pub struct ApplyGoalArgs {
    pub goal: ApplyGoal,
    pub should_apply_locally: bool,
    pub no_keys: bool,
    pub substitute_on_destination: bool,
    pub reboot: bool,
    pub host_platform: Arc<str>,
    pub handle_unreachable: HandleUnreachable,
}

pub enum Goal {
    Apply(ApplyGoalArgs),
    Build,
}

type SharedStepNodeFuture<NodeOutput> =
    Shared<Pin<Box<dyn Future<Output = Result<Arc<NodeOutput>, Arc<HiveLibError>>> + Send>>>;

#[derive(Clone)]
pub struct StepNode<NodeOutput> {
    output_future: SharedStepNodeFuture<NodeOutput>,
}

impl<Output: Send + Sync + Clone + 'static> StepNode<Output> {
    fn spawn<Fut>(work: impl FnOnce() -> Fut + Send + 'static) -> Self
    where
        Fut: Future<Output = Result<Output, Arc<HiveLibError>>> + Send,
    {
        let output_future = (Box::pin(async move { work().await.map(Arc::new) })
            as Pin<Box<dyn Future<Output = Result<_, _>> + Send>>)
            .shared();

        Self { output_future }
    }

    pub async fn get_output(&self) -> Result<Arc<Output>, Arc<HiveLibError>> {
        self.output_future.clone().await
    }
}

#[derive(Clone)]
pub enum PushSource {
    Evaluation,
    Build,
    KeyAgent,
}

#[derive(Clone)]
pub enum PlanStep {
    Ping,
    Evaluate {
        cached: Option<SafeStorePath<String>>,
    },
    PushOutput {
        source: PushSource,
        substitute_on_destination: bool,
    },
    Build {
        uses_daemon: bool,
    },
    SwitchToConfiguration {
        goal: SwitchToConfigurationGoal,
        reboot: bool,
    },
    Keys {
        keys: Vec<Arc<super::steps::keys::Key>>,
    },
}

#[derive(Clone)]
pub struct EvaluationNodeOutput(pub SafeStorePath<String>);

#[derive(Clone)]
pub struct PushBuildOutput(pub SafeStorePath<String>);

#[derive(Clone)]
pub struct PushDerivationOutput(pub SafeStorePath<String>);

#[derive(Clone)]
pub struct PushKeyAgentOutput(pub SafeStorePath<String>);

#[derive(Clone)]
pub struct BuildNodeOutput(pub SafeStorePath<String>);

#[derive(Clone)]
pub struct SwitchToConfigurationOutput(pub ());

#[derive(Clone)]
pub struct KeysOutput(pub ());

#[derive(Clone)]
pub enum AnyNodeOutput {
    Ping(Arc<SharedTarget>),
    Derivation(Arc<EvaluationNodeOutput>),
    PushKeyAgent(Arc<PushKeyAgentOutput>),
    PushDerivation(Arc<PushDerivationOutput>),
    PushBuildOutput(Arc<PushBuildOutput>),
    Build(Arc<BuildNodeOutput>),
    SwitchToConfiguration(Arc<SwitchToConfigurationOutput>),
    Keys(Arc<KeysOutput>),
}

pub type AnyNode = StepNode<AnyNodeOutput>;

macro_rules! impl_any_node_output {
    ($variant:ident, $ty:ty) => {
        impl TryFrom<AnyNodeOutput> for $ty {
            type Error = ();

            fn try_from(n: AnyNodeOutput) -> Result<Self, Self::Error> {
                match n {
                    AnyNodeOutput::$variant(v) => Ok((&*v).clone()),
                    _ => Err(()),
                }
            }
        }
    };
}

impl_any_node_output!(Ping, SharedTarget);
impl_any_node_output!(Derivation, EvaluationNodeOutput);
impl_any_node_output!(PushKeyAgent, PushKeyAgentOutput);
impl_any_node_output!(PushDerivation, PushDerivationOutput);
impl_any_node_output!(PushBuildOutput, PushBuildOutput);
impl_any_node_output!(Build, BuildNodeOutput);
impl_any_node_output!(SwitchToConfiguration, SwitchToConfigurationOutput);
impl_any_node_output!(Keys, KeysOutput);

pub trait AnyNodeOutputSliceExt {
    fn require<T: TryFrom<AnyNodeOutput>>(&self) -> Result<T, HiveLibError>;
}

impl AnyNodeOutputSliceExt for [AnyNodeOutput] {
    fn require<T: TryFrom<AnyNodeOutput>>(&self) -> Result<T, HiveLibError> {
        self.iter()
            .find_map(|output| T::try_from(output.clone()).ok())
            .ok_or_else(|| HiveLibError::MissingStepOutput)
    }
}

pub struct PlanGraph {
    graph: DiGraph<Option<Step>, ()>,
    pub context: Arc<Context>,
}

impl PlanGraph {
    pub fn get_sink_step(self) -> Result<StepNode<AnyNodeOutput>, HiveLibError> {
        let sinks = self.graph.externals(Direction::Outgoing).collect_vec();
        let mut compilation = self.compile();

        if sinks.len() > 1 {
            todo!("error here!!! more than one sink");
        }

        let first = sinks.first().unwrap();

        Ok(compilation.remove(first).unwrap())
    }

    fn new_build_plan(
        modifiers: &SubCommandModifiers,
        cached_evaluation: Option<&SafeStorePath<String>>,
    ) -> DiGraph<Option<Step>, ()> {
        let mut graph = DiGraph::<Option<Step>, ()>::new();

        let evaluate_idx =
            (modifiers.experimental_nix_client || cached_evaluation.is_some()).then(|| {
                graph.add_node(Some(Step::Evaluate(Evaluate {
                    cached_evaluation: cached_evaluation.cloned(),
                })))
            });

        let build_idx = graph.add_node(Some(Step::Build(Build {
            metadata: if modifiers.experimental_nix_client {
                BuildMetadata::BuildWithNixDaemon
            } else {
                BuildMetadata::NixCommand(NixCommandBuildMetadata::Locally)
            },
        })));

        // if evaluation was added, add a evaluate -> build edge
        if let Some(evaluate_idx) = evaluate_idx {
            graph.add_edge(evaluate_idx, build_idx, ());
        }

        graph
    }

    fn add_key_nodes(
        args: &ApplyGoalArgs,
        node: &Node,
        ping_idx: Option<NodeIndex>,
        graph: &mut DiGraph<Option<Step>, ()>,
    ) -> (Option<NodeIndex>, Option<NodeIndex>) {
        let ApplyGoalArgs {
            goal,
            substitute_on_destination,
            host_platform,
            ..
        } = args;

        let (pre_keys, post_keys) = match goal {
            ApplyGoal::SwitchToConfiguration(SwitchToConfigurationGoal::Switch) => node
                .keys
                .clone()
                .into_iter()
                .partition(|x| matches!(x.upload_at, UploadKeyAt::PreActivation)),
            ApplyGoal::Keys => (node.keys.clone(), Vec::new()),
            ApplyGoal::Build | ApplyGoal::Push | ApplyGoal::SwitchToConfiguration(_) => {
                unreachable!("apply_plan_keys called with non-key goal: {:?}", goal)
            }
        };

        let push_agent_idx = (!pre_keys.is_empty() || !post_keys.is_empty()).then(|| {
            let idx = graph.add_node(Some(Step::PushKeyAgent(PushKeyAgent {
                substitute_on_destination: *substitute_on_destination,
                host_platform: host_platform.clone(),
            })));

            if let Some(ping_idx) = ping_idx {
                graph.add_edge(ping_idx, idx, ());
            }

            idx
        });

        let pre_step = (!pre_keys.is_empty()).then(|| {
            let idx = graph.add_node(Some(Step::Keys(Keys {
                keys: pre_keys,
                privilege_escalation_command: node.privilege_escalation_command.clone(),
            })));

            if let Some(ping_idx) = ping_idx {
                graph.add_edge(ping_idx, idx, ());
            }

            if let Some(push_agent_idx) = push_agent_idx {
                graph.add_edge(push_agent_idx, idx, ());
            }

            idx
        });

        let post_step = (!post_keys.is_empty()).then(|| {
            let idx = graph.add_node(Some(Step::Keys(Keys {
                keys: post_keys,
                privilege_escalation_command: node.privilege_escalation_command.clone(),
            })));

            if let Some(ping_idx) = ping_idx {
                graph.add_edge(ping_idx, idx, ());
            }

            if let Some(push_agent_idx) = push_agent_idx {
                graph.add_edge(push_agent_idx, idx, ());
            }

            idx
        });

        (pre_step, post_step)
    }

    fn new_apply_plan(
        args: &ApplyGoalArgs,
        node: &Node,
        modifiers: &SubCommandModifiers,
        cached_evaluation: Option<&SafeStorePath<String>>,
    ) -> DiGraph<Option<Step>, ()> {
        let ApplyGoalArgs {
            goal,
            should_apply_locally,
            no_keys,
            substitute_on_destination,
            reboot,
            ..
        } = args;

        let mut graph = DiGraph::new();
        let build_will_have_target = node.build_remotely && !*should_apply_locally;

        let ping = (!*should_apply_locally).then(|| {
            graph.add_node(Some(Step::Ping(Ping {
                target: SharedTarget(Arc::new(RwLock::new(node.target.clone()))),
            })))
        });

        // let (pre_keys, post_keys) = Self::add_key_nodes(args, node, ping, &mut graph);

        let needs_separate_eval = match goal {
            ApplyGoal::Keys => false,
            // a `.drv` must be known before so it can be pushed
            ApplyGoal::Push => true,
            // the experimental nix client still requires the path to build
            _ if modifiers.experimental_nix_client => true,
            _ if cached_evaluation.is_some() => true,
            // only evaluate if the build step will require a real `.drv` on the remote system
            // or if it can use an attribute that exists on the local host directly
            _ => build_will_have_target,
        };

        let evaluate_idx = needs_separate_eval.then(|| {
            graph.add_node(Some(Step::Evaluate(Evaluate {
                cached_evaluation: cached_evaluation.cloned(),
            })))
        });

        let push_drv_idx = (!matches!(goal, ApplyGoal::Keys)
            && !should_apply_locally
            && (node.build_remotely || matches!(goal, ApplyGoal::Push)))
        .then(|| {
            graph.add_node(Some(Step::PushOutput(PushOutput {
                kind: PushOutputKind::Evaluation,
                substitute_on_destination: *substitute_on_destination,
            })))
        });

        // if there are both a Evaluate step and a push evaluate step,
        // connect Evaluate -> PushEvaluate
        if let Some(push_drv_idx) = push_drv_idx
            && let Some(evaluate_idx) = evaluate_idx
        {
            graph.add_edge(evaluate_idx, push_drv_idx, ());
        }

        // connect Ping -> PushEvaluate
        if let Some(ping_idx) = ping
            && let Some(push_drv_idx) = push_drv_idx
        {
            graph.add_edge(ping_idx, push_drv_idx, ());
        }

        let build_idx = (!matches!(goal, ApplyGoal::Keys | ApplyGoal::Push)).then(|| {
            let idx = graph.add_node(Some(Step::Build(Build {
                metadata: if modifiers.experimental_nix_client {
                    BuildMetadata::BuildWithNixDaemon
                } else {
                    BuildMetadata::NixCommand(if build_will_have_target {
                        NixCommandBuildMetadata::Remotely
                    } else {
                        NixCommandBuildMetadata::Locally
                    })
                },
            })));

            // if there is a push evaluate step, connect PushEvaluate -> Build
            if let Some(push_output_idx) = push_drv_idx {
                graph.add_edge(push_output_idx, idx, ());
            }

            idx
        });

        // if `build_will_have_target`, connect Ping -> Build
        if let Some(ping_idx) = ping
            && let Some(build_idx) = build_idx
            && build_will_have_target
        {
            graph.add_edge(ping_idx, build_idx, ());
        }

        let push_output_idx = (!node.build_remotely
            && !should_apply_locally
            && !matches!(goal, ApplyGoal::Keys | ApplyGoal::Push))
        .then(|| {
            graph.add_node(Some(Step::PushOutput(PushOutput {
                kind: PushOutputKind::Build,
                substitute_on_destination: *substitute_on_destination,
            })))
        });

        // connect Ping -> PushBuild
        if let Some(ping_idx) = ping
            && let Some(push_output_idx) = push_output_idx
        {
            graph.add_edge(ping_idx, push_output_idx, ());
        }

        // if there are both a Build step and a push output step,
        // connect Build -> PushOutput
        if let Some(push_output_idx) = push_output_idx
            && let Some(build_idx) = build_idx
        {
            graph.add_edge(build_idx, push_output_idx, ());
        }

        let key_indexes = (!*no_keys
            && matches!(
                &goal,
                ApplyGoal::Keys
                    | ApplyGoal::SwitchToConfiguration(SwitchToConfigurationGoal::Switch)
            ))
        .then(|| {
            let (pre, post) = Self::add_key_nodes(args, node, ping, &mut graph);

            if let Some(push_output_idx) = push_output_idx
                && let Some(pre_idx) = pre
            {
                graph.add_edge(push_output_idx, pre_idx, ());
            }

            if let Some(build_idx) = build_idx
                && let Some(pre_idx) = pre
            {
                graph.add_edge(build_idx, pre_idx, ());
            }

            (pre, post)
        });

        let activate = if let ApplyGoal::SwitchToConfiguration(goal) = goal {
            let idx = graph.add_node(Some(Step::SwitchToConfiguration(SwitchToConfiguration {
                privilege_escalation_command: node.privilege_escalation_command.clone(),
                goal: *goal,
                reboot: *reboot,
            })));

            // add Ping -> SwitchToConfiguration if applying locally
            if let Some(ping_idx) = ping {
                graph.add_edge(ping_idx, idx, ());
            }

            // if we are pushing the built output, add PushBuild ->
            // SwitchToConfiguration, otherwise, we have built it locally and we
            // should only add Build -> SwitchToConfiguration
            if let Some(push_output_idx) = push_output_idx {
                graph.add_edge(push_output_idx, idx, ());
            } else if let Some(build_idx) = build_idx {
                graph.add_edge(build_idx, idx, ());
            }

            // if there are any pre-keys, add Pre Keys -> SwitchToConfiguration
            if let Some((Some(pre), _)) = key_indexes {
                graph.add_edge(pre, idx, ());
            }

            Some(
                graph.add_node(Some(Step::SwitchToConfiguration(SwitchToConfiguration {
                    goal: *goal,
                    reboot: *reboot,
                    privilege_escalation_command: node.privilege_escalation_command.clone(),
                }))),
            )
        } else {
            None
        };

        // add SwitchToConfiguration -> Post Keys
        if let Some((_, Some(post))) = key_indexes
            && let Some(activate) = activate
        {
            graph.add_edge(activate, post, ());
        }

        graph
    }

    pub fn new(
        node: &Node,
        name: Name,
        goal: &'_ Goal,
        hive_location: Arc<HiveLocation>,
        should_quit: Arc<AtomicBool>,
        modifiers: SubCommandModifiers,
        cached_evaluation: Option<&SafeStorePath<String>>,
    ) -> Self {
        let graph = match goal {
            Goal::Build => Self::new_build_plan(&modifiers, cached_evaluation),
            Goal::Apply(args) => Self::new_apply_plan(args, node, &modifiers, cached_evaluation),
        };

        Self {
            graph,
            context: Context {
                hive_location,
                modifiers,
                should_quit,
                name,
            }
            .into(),
        }
    }

    #[must_use]
    pub fn compile(mut self) -> HashMap<NodeIndex, AnyNode> {
        let mut built: HashMap<NodeIndex, AnyNode> = HashMap::new();

        for index in toposort(&self.graph, None).expect("plan graph to be acylic") {
            let step = self.graph[index].take().unwrap();

            let parents = self
                .graph
                .edges_directed(index, Direction::Incoming)
                .map(|x| built[&x.source()].clone())
                .collect_vec();

            let ctx_cloned = self.context.clone();

            let node = StepNode::spawn(move || async move {
                let inputs =
                    futures::future::join_all(parents.into_iter().map(|parent| async move {
                        let node = parent;
                        node.get_output().await
                    }))
                    .await
                    .into_iter()
                    .collect::<Result<Vec<Arc<AnyNodeOutput>>, Arc<HiveLibError>>>()?
                    .into_iter()
                    .map(|output| (*output).clone())
                    .collect::<Vec<AnyNodeOutput>>();

                Step::execute(step, inputs, ctx_cloned)
                    .await
                    .map_err(Arc::new)
            });

            built.insert(index, node);
        }

        built
    }
}

impl Display for PlanGraph {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for step in toposort(&self.graph, None).expect("plan graph to be acylic") {
            write!(
                f,
                "{}\n",
                self.graph[step]
                    .as_ref()
                    .expect("step index to exist in node")
            )?;
        }

        Ok(())
    }
}

// #[cfg(test)]
// mod tests {
//     use tokio::sync::RwLock;
//
//     use crate::{
//         SafeStorePath, SubCommandModifiers, function_name, get_test_path,
//         hive::{
//             executor::{BuildOutputHandle, EvaluationOutputHandle, OutputHandle},
//             node::{
//                 ApplyGoal, HandleUnreachable, Name, Node, SharedTarget, Step,
//                 SwitchToConfigurationGoal,
//             },
//             plan::{ApplyGoalArgs, Goal, plan_for_node},
//             steps::{
//                 activate::SwitchToConfiguration,
//                 build::{Build, BuildMetadata, NixCommandBuildMetadata},
//                 evaluate::Evaluate,
//                 keys::{Key, Keys, PushKeyAgent, Source, UploadKeyAt},
//                 ping::Ping,
//                 push::{PushOutput, PushOutputHandle},
//             },
//         },
//         location,
//     };
//     use std::path::PathBuf;
//     use std::{
//         env,
//         sync::{Arc, atomic::AtomicBool},
//     };
//
//     fn new_key(upload_at: &UploadKeyAt) -> Key {
//         Key {
//             upload_at: upload_at.clone(),
//             source: Source::String(match upload_at {
//                 UploadKeyAt::PreActivation => "pre".into(),
//                 UploadKeyAt::PostActivation => "post".into(),
//                 UploadKeyAt::NoFilter => "none".into(),
//             }),
//             ..Default::default()
//         }
//     }
//
//     #[tokio::test]
//     async fn order_build() {
//         let location = location!(get_test_path!());
//         let node = Node {
//             build_remotely: false,
//             ..Default::default()
//         };
//         let name = &Name(function_name!().into());
//         let should_quit = Arc::new(AtomicBool::new(false));
//         let plan = plan_for_node(
//             &node,
//             name.clone(),
//             &Goal::Build,
//             location.into(),
//             &SubCommandModifiers::default(),
//             should_quit,
//             None,
//         );
//
//         assert_eq!(
//             plan.steps,
//             vec![
//                 Build {
//                     output: BuildOutputHandle::new(),
//                     metadata: BuildMetadata::NixCommand(NixCommandBuildMetadata::Locally {
//                         cached_derivation: None,
//                     }),
//                 }
//                 .into()
//             ]
//         );
//         assert!(!plan.greedy_evaluate);
//     }
//
//     #[tokio::test]
//     async fn order_apply_build() {
//         let location = location!(get_test_path!());
//         let node = Node {
//             build_remotely: true,
//             ..Default::default()
//         };
//         let name = &Name(function_name!().into());
//         let should_quit = Arc::new(AtomicBool::new(false));
//         let target = SharedTarget(Arc::new(RwLock::new(node.target.clone())));
//         let plan = plan_for_node(
//             &node,
//             name.clone(),
//             &Goal::Apply(ApplyGoalArgs {
//                 goal: ApplyGoal::Build,
//                 should_apply_locally: false,
//                 no_keys: true,
//                 substitute_on_destination: true,
//                 reboot: false,
//                 host_platform: "x86_64-linux".into(),
//                 handle_unreachable: HandleUnreachable::default(),
//             }),
//             location.clone().into(),
//             &SubCommandModifiers::default(),
//             should_quit.clone(),
//             None,
//         );
//
//         assert_eq!(
//             plan.steps,
//             vec![
//                 Ping {
//                     target: target.clone()
//                 }
//                 .into(),
//                 Evaluate {
//                     output: EvaluationOutputHandle::new(),
//                 }
//                 .into(),
//                 PushOutput {
//                     substitute_on_destination: true,
//                     target: target.clone(),
//                     path: PushOutputHandle::Evaluation(EvaluationOutputHandle::new()),
//                 }
//                 .into(),
//                 Build {
//                     output: BuildOutputHandle::new(),
//                     metadata: BuildMetadata::NixCommand(NixCommandBuildMetadata::Remotely {
//                         target: target.clone(),
//                         derivation: EvaluationOutputHandle::new(),
//                     }),
//                 }
//                 .into(),
//             ]
//         );
//         assert!(plan.greedy_evaluate);
//
//         let node = Node {
//             build_remotely: false,
//             ..Default::default()
//         };
//         let plan = plan_for_node(
//             &node,
//             name.clone(),
//             &Goal::Apply(ApplyGoalArgs {
//                 goal: ApplyGoal::Build,
//                 should_apply_locally: false,
//                 no_keys: true,
//                 substitute_on_destination: true,
//                 reboot: false,
//                 host_platform: "x86_64-linux".into(),
//                 handle_unreachable: HandleUnreachable::default(),
//             }),
//             location.into(),
//             &SubCommandModifiers::default(),
//             should_quit,
//             None,
//         );
//
//         assert_eq!(
//             plan.steps,
//             vec![
//                 Ping {
//                     target: target.clone()
//                 }
//                 .into(),
//                 Build {
//                     output: BuildOutputHandle::new(),
//                     metadata: BuildMetadata::NixCommand(NixCommandBuildMetadata::Locally {
//                         cached_derivation: None,
//                     }),
//                 }
//                 .into(),
//                 PushOutput {
//                     substitute_on_destination: true,
//                     target,
//                     path: PushOutputHandle::Build(BuildOutputHandle::new()),
//                 }
//                 .into(),
//             ]
//         );
//         assert!(!plan.greedy_evaluate);
//     }
//
//     #[tokio::test]
//     async fn order_keys_only() {
//         let location = location!(get_test_path!());
//         let node = Node {
//             keys: vec![
//                 new_key(&UploadKeyAt::PreActivation).into(),
//                 new_key(&UploadKeyAt::PostActivation).into(),
//                 new_key(&UploadKeyAt::PreActivation).into(),
//                 new_key(&UploadKeyAt::PostActivation).into(),
//             ],
//             ..Default::default()
//         };
//         let name = &Name(function_name!().into());
//         let should_quit = Arc::new(AtomicBool::new(false));
//         let target = SharedTarget(Arc::new(RwLock::new(node.target.clone())));
//         let plan_apply_keys = plan_for_node(
//             &node,
//             name.clone(),
//             &Goal::Apply(ApplyGoalArgs {
//                 goal: ApplyGoal::Keys,
//                 should_apply_locally: false,
//                 no_keys: false,
//                 substitute_on_destination: true,
//                 reboot: false,
//                 host_platform: "x86_64-linux".into(),
//                 handle_unreachable: HandleUnreachable::default(),
//             }),
//             location.into(),
//             &SubCommandModifiers::default(),
//             should_quit,
//             None,
//         );
//
//         assert_eq!(
//             plan_apply_keys.steps,
//             vec![
//                 Ping {
//                     target: target.clone()
//                 }
//                 .into(),
//                 PushKeyAgent {
//                     substitute_on_destination: true,
//                     target: Some(target.clone()),
//                     host_platform: node.host_platform.clone(),
//                     key_agent_directory: OutputHandle::new(),
//                 }
//                 .into(),
//                 Keys {
//                     target: Some(target),
//                     // test that all keys are included
//                     keys: node.keys.clone(),
//                     privilege_escalation_command: node.privilege_escalation_command,
//                     key_agent_directory: OutputHandle::new(),
//                 }
//                 .into(),
//             ]
//         );
//         assert!(!plan_apply_keys.greedy_evaluate);
//     }
//
//     #[tokio::test]
//     async fn order_key_split() {
//         let location = location!(get_test_path!());
//         let node = Node {
//             keys: vec![
//                 new_key(&UploadKeyAt::PreActivation).into(),
//                 new_key(&UploadKeyAt::PostActivation).into(),
//                 new_key(&UploadKeyAt::PreActivation).into(),
//                 new_key(&UploadKeyAt::PostActivation).into(),
//             ],
//             ..Default::default()
//         };
//         let name = &Name(function_name!().into());
//         let should_quit = Arc::new(AtomicBool::new(false));
//
//         // Test that keys are split by their `upload_at`, also tests that key
//         // step's `target` abides by should_apply_locally
//         let plan_activate_with_keys = plan_for_node(
//             &node,
//             name.clone(),
//             &Goal::Apply(ApplyGoalArgs {
//                 goal: ApplyGoal::SwitchToConfiguration(
//                     crate::hive::node::SwitchToConfigurationGoal::Switch,
//                 ),
//                 should_apply_locally: true,
//                 no_keys: false,
//                 substitute_on_destination: true,
//                 reboot: false,
//                 host_platform: "x86_64-linux".into(),
//                 handle_unreachable: HandleUnreachable::default(),
//             }),
//             location.into(),
//             &SubCommandModifiers::default(),
//             should_quit,
//             None,
//         );
//
//         assert_eq!(
//             plan_activate_with_keys
//                 .steps
//                 .into_iter()
//                 .filter(|x| matches!(
//                     x,
//                     Step::Keys(Keys { .. }) | Step::PushKeyAgent(PushKeyAgent { .. })
//                 ))
//                 .collect::<Vec<Step>>(),
//             vec![
//                 PushKeyAgent {
//                     substitute_on_destination: true,
//                     target: None,
//                     host_platform: node.host_platform.clone(),
//                     key_agent_directory: OutputHandle::new(),
//                 }
//                 .into(),
//                 Keys {
//                     target: None,
//                     keys: node
//                         .keys
//                         .iter()
//                         .filter(|key| matches!(key.upload_at, UploadKeyAt::PreActivation))
//                         .cloned()
//                         .collect::<Vec<_>>(),
//                     privilege_escalation_command: node.privilege_escalation_command.clone(),
//                     key_agent_directory: OutputHandle::new(),
//                 }
//                 .into(),
//                 Keys {
//                     target: None,
//                     keys: node
//                         .keys
//                         .iter()
//                         .filter(|key| matches!(key.upload_at, UploadKeyAt::PostActivation))
//                         .cloned()
//                         .collect::<Vec<_>>(),
//                     privilege_escalation_command: node.privilege_escalation_command.clone(),
//                     key_agent_directory: OutputHandle::new(),
//                 }
//                 .into(),
//             ]
//         );
//     }
//
//     #[tokio::test]
//     async fn order_push_only() {
//         let location = location!(get_test_path!());
//         let node = Node::default();
//         let name = &Name(function_name!().into());
//         let should_quit = Arc::new(AtomicBool::new(false));
//         let target = SharedTarget(Arc::new(RwLock::new(node.target.clone())));
//         let plan = plan_for_node(
//             &node,
//             name.clone(),
//             &Goal::Apply(ApplyGoalArgs {
//                 goal: ApplyGoal::Push,
//                 should_apply_locally: false,
//                 no_keys: false,
//                 substitute_on_destination: true,
//                 reboot: false,
//                 host_platform: "x86_64-linux".into(),
//                 handle_unreachable: HandleUnreachable::default(),
//             }),
//             location.into(),
//             &SubCommandModifiers::default(),
//             should_quit,
//             None,
//         );
//
//         assert_eq!(
//             plan.steps,
//             vec![
//                 Ping {
//                     target: target.clone()
//                 }
//                 .into(),
//                 Evaluate {
//                     output: EvaluationOutputHandle::new(),
//                 }
//                 .into(),
//                 PushOutput {
//                     substitute_on_destination: true,
//                     target,
//                     path: PushOutputHandle::Evaluation(EvaluationOutputHandle::new()),
//                 }
//                 .into()
//             ]
//         );
//         assert!(plan.greedy_evaluate);
//     }
//
//     #[tokio::test]
//     async fn order_remote_build() {
//         let location = location!(get_test_path!());
//         let node = Node {
//             build_remotely: true,
//             ..Default::default()
//         };
//         let name = &Name(function_name!().into());
//         let should_quit = Arc::new(AtomicBool::new(false));
//         let target = SharedTarget(Arc::new(RwLock::new(node.target.clone())));
//         let plan = plan_for_node(
//             &node,
//             name.clone(),
//             &Goal::Apply(ApplyGoalArgs {
//                 goal: ApplyGoal::SwitchToConfiguration(SwitchToConfigurationGoal::Switch),
//                 should_apply_locally: false,
//                 no_keys: false,
//                 substitute_on_destination: true,
//                 reboot: false,
//                 host_platform: "x86_64-linux".into(),
//                 handle_unreachable: HandleUnreachable::default(),
//             }),
//             location.into(),
//             &SubCommandModifiers::default(),
//             should_quit,
//             None,
//         );
//
//         assert_eq!(
//             plan.steps,
//             vec![
//                 Ping {
//                     target: target.clone()
//                 }
//                 .into(),
//                 Evaluate {
//                     output: EvaluationOutputHandle::new(),
//                 }
//                 .into(),
//                 PushOutput {
//                     substitute_on_destination: true,
//                     target: target.clone(),
//                     path: PushOutputHandle::Evaluation(EvaluationOutputHandle::new()),
//                 }
//                 .into(),
//                 Build {
//                     output: BuildOutputHandle::new(),
//                     metadata: BuildMetadata::NixCommand(NixCommandBuildMetadata::Remotely {
//                         target: target.clone(),
//                         derivation: EvaluationOutputHandle::new(),
//                     }),
//                 }
//                 .into(),
//                 SwitchToConfiguration {
//                     goal: SwitchToConfigurationGoal::Switch,
//                     reboot: false,
//                     target: Some(target),
//                     privilege_escalation_command: node.privilege_escalation_command,
//                     top_level: BuildOutputHandle::new(),
//                 }
//                 .into(),
//             ]
//         );
//         assert!(plan.greedy_evaluate);
//     }
//
//     #[tokio::test]
//     async fn order_nokeys() {
//         let location = location!(get_test_path!());
//         let node = Node {
//             keys: vec![Key::default().into(), Key::default().into()],
//             build_remotely: true,
//             ..Default::default()
//         };
//         let name = &Name(function_name!().into());
//         let should_quit = Arc::new(AtomicBool::new(false));
//         let target = SharedTarget(Arc::new(RwLock::new(node.target.clone())));
//         let plan = plan_for_node(
//             &node,
//             name.clone(),
//             &Goal::Apply(ApplyGoalArgs {
//                 goal: ApplyGoal::SwitchToConfiguration(SwitchToConfigurationGoal::Switch),
//                 should_apply_locally: false,
//                 no_keys: true,
//                 substitute_on_destination: true,
//                 reboot: false,
//                 host_platform: "x86_64-linux".into(),
//                 handle_unreachable: HandleUnreachable::default(),
//             }),
//             location.into(),
//             &SubCommandModifiers::default(),
//             should_quit,
//             None,
//         );
//
//         assert_eq!(
//             plan.steps,
//             vec![
//                 Ping {
//                     target: target.clone()
//                 }
//                 .into(),
//                 Evaluate {
//                     output: EvaluationOutputHandle::new(),
//                 }
//                 .into(),
//                 PushOutput {
//                     substitute_on_destination: true,
//                     target: target.clone(),
//                     path: PushOutputHandle::Evaluation(EvaluationOutputHandle::new()),
//                 }
//                 .into(),
//                 Build {
//                     output: BuildOutputHandle::new(),
//                     metadata: BuildMetadata::NixCommand(NixCommandBuildMetadata::Remotely {
//                         target: target.clone(),
//                         derivation: EvaluationOutputHandle::new(),
//                     }),
//                 }
//                 .into(),
//                 SwitchToConfiguration {
//                     goal: SwitchToConfigurationGoal::Switch,
//                     reboot: false,
//                     target: Some(target),
//                     privilege_escalation_command: node.privilege_escalation_command,
//                     top_level: BuildOutputHandle::new(),
//                 }
//                 .into(),
//             ]
//         );
//         assert!(plan.greedy_evaluate);
//     }
//
//     #[tokio::test]
//     async fn order_should_apply_locally() {
//         let location = location!(get_test_path!());
//         let node = Node::default();
//         let name = &Name(function_name!().into());
//         let should_quit = Arc::new(AtomicBool::new(false));
//         let plan = plan_for_node(
//             &node,
//             name.clone(),
//             &Goal::Apply(ApplyGoalArgs {
//                 goal: ApplyGoal::SwitchToConfiguration(SwitchToConfigurationGoal::Switch),
//                 should_apply_locally: true,
//                 no_keys: true,
//                 substitute_on_destination: true,
//                 reboot: false,
//                 host_platform: "x86_64-linux".into(),
//                 handle_unreachable: HandleUnreachable::default(),
//             }),
//             location.into(),
//             &SubCommandModifiers::default(),
//             should_quit,
//             None,
//         );
//
//         assert_eq!(
//             plan.steps,
//             vec![
//                 Build {
//                     output: BuildOutputHandle::new(),
//                     metadata: BuildMetadata::NixCommand(NixCommandBuildMetadata::Locally {
//                         cached_derivation: None,
//                     }),
//                 }
//                 .into(),
//                 SwitchToConfiguration {
//                     goal: SwitchToConfigurationGoal::Switch,
//                     reboot: false,
//                     target: None,
//                     privilege_escalation_command: node.privilege_escalation_command,
//                     top_level: BuildOutputHandle::new(),
//                 }
//                 .into(),
//             ]
//         );
//         assert!(!plan.greedy_evaluate);
//     }
//
//     #[tokio::test]
//     async fn order_build_cached_evaluation() {
//         let location = location!(get_test_path!());
//         let node = Node::default();
//         let name = &Name(function_name!().into());
//         let should_quit = Arc::new(AtomicBool::new(false));
//         let cached = SafeStorePath::<String>::from_absolute_path(
//             b"/nix/store/0cg1bwya4a0r5y9vbi5c79jsvgmicg1p-name",
//         )
//         .unwrap();
//         let plan = plan_for_node(
//             &node,
//             name.clone(),
//             &Goal::Build,
//             location.into(),
//             &SubCommandModifiers::default(),
//             should_quit,
//             Some(&cached),
//         );
//
//         // a cached evaluation means no Evaluate step is scheduled, and the
//         // local build reuses the known derivation path.
//         assert_eq!(
//             plan.steps,
//             vec![
//                 Build {
//                     output: BuildOutputHandle::new(),
//                     metadata: BuildMetadata::NixCommand(NixCommandBuildMetadata::Locally {
//                         cached_derivation: Some(EvaluationOutputHandle::new_known(cached)),
//                     }),
//                 }
//                 .into()
//             ]
//         );
//         assert!(!plan.greedy_evaluate);
//     }
//
//     #[tokio::test]
//     async fn order_build_experimental_nix_client() {
//         let location = location!(get_test_path!());
//         let node = Node::default();
//         let name = &Name(function_name!().into());
//         let should_quit = Arc::new(AtomicBool::new(false));
//         let plan = plan_for_node(
//             &node,
//             name.clone(),
//             &Goal::Build,
//             location.into(),
//             &SubCommandModifiers {
//                 experimental_nix_client: true,
//                 ..Default::default()
//             },
//             should_quit,
//             None,
//         );
//
//         // the experimental nix client needs a real `.drv`, so an Evaluate
//         // step is scheduled even for a local Build.
//         assert_eq!(
//             plan.steps,
//             vec![
//                 Evaluate {
//                     output: EvaluationOutputHandle::new(),
//                 }
//                 .into(),
//                 Build {
//                     output: BuildOutputHandle::new(),
//                     metadata: BuildMetadata::BuildWithNixDaemon {
//                         target: None,
//                         derivation: EvaluationOutputHandle::new(),
//                     },
//                 }
//                 .into(),
//             ]
//         );
//         assert!(plan.greedy_evaluate);
//     }
//
//     #[tokio::test]
//     async fn order_remote_build_experimental_nix_client() {
//         let location = location!(get_test_path!());
//         let node = Node {
//             build_remotely: true,
//             ..Default::default()
//         };
//         let name = &Name(function_name!().into());
//         let should_quit = Arc::new(AtomicBool::new(false));
//         let target = SharedTarget(Arc::new(RwLock::new(node.target.clone())));
//         let plan = plan_for_node(
//             &node,
//             name.clone(),
//             &Goal::Apply(ApplyGoalArgs {
//                 goal: ApplyGoal::SwitchToConfiguration(SwitchToConfigurationGoal::Switch),
//                 should_apply_locally: false,
//                 no_keys: true,
//                 substitute_on_destination: true,
//                 reboot: false,
//                 host_platform: "x86_64-linux".into(),
//                 handle_unreachable: HandleUnreachable::default(),
//             }),
//             location.into(),
//             &SubCommandModifiers {
//                 experimental_nix_client: true,
//                 ..Default::default()
//             },
//             should_quit,
//             None,
//         );
//
//         assert_eq!(
//             plan.steps,
//             vec![
//                 Ping {
//                     target: target.clone()
//                 }
//                 .into(),
//                 Evaluate {
//                     output: EvaluationOutputHandle::new(),
//                 }
//                 .into(),
//                 PushOutput {
//                     substitute_on_destination: true,
//                     target: target.clone(),
//                     path: PushOutputHandle::Evaluation(EvaluationOutputHandle::new()),
//                 }
//                 .into(),
//                 Build {
//                     output: BuildOutputHandle::new(),
//                     metadata: BuildMetadata::BuildWithNixDaemon {
//                         target: Some(target.clone()),
//                         derivation: EvaluationOutputHandle::new(),
//                     },
//                 }
//                 .into(),
//                 SwitchToConfiguration {
//                     goal: SwitchToConfigurationGoal::Switch,
//                     reboot: false,
//                     target: Some(target),
//                     privilege_escalation_command: node.privilege_escalation_command,
//                     top_level: BuildOutputHandle::new(),
//                 }
//                 .into(),
//             ]
//         );
//         assert!(plan.greedy_evaluate);
//     }
// }
