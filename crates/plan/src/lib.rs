use std::{collections::HashMap, sync::{Arc, atomic::AtomicBool}};

use enum_dispatch::enum_dispatch;
use serde::{Deserialize, Serialize};
use wire_core::{
    SubCommandModifiers, hive::{
        HiveLocation, node::{Context, Derivation, Name, StepState, SwitchToConfigurationGoal, Target}, steps::keys::UploadKeyAt
    }
};
use wire_execute::{
    Build, Evaluate, ExecuteStep, Keys, Ping, PushBuildOutput, PushEvaluatedOutput, PushKeyAgent,
    activate::SwitchToConfiguration,
};
use wire_keys::{Key};

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq, Hash)]
pub struct NodeRepr {
    #[serde(rename = "target")]
    pub target: Target,

    #[serde(rename = "buildOnTarget")]
    pub build_remotely: bool,

    #[serde(rename = "allowLocalDeployment")]
    pub allow_local_deployment: bool,

    #[serde(default)]
    pub tags: im::HashSet<String>,

    #[serde(rename(deserialize = "_keys", serialize = "keys"))]
    pub keys: Vec<Key>,

    #[serde(rename(deserialize = "_hostPlatform", serialize = "host_platform"))]
    pub host_platform: Arc<str>,

    #[serde(rename(
        deserialize = "privilegeEscalationCommand",
        serialize = "privilege_escalation_command"
    ))]
    pub privilege_escalation_command: im::Vector<Arc<str>>,
}

pub struct Node {
    pub target: Arc<Target>,
    pub build_remotely: bool,
    pub allow_local_deployment: bool,
    pub tags: im::HashSet<String>,
    pub keys: Vec<Arc<Key>>,
    pub host_platform: Arc<str>,
    pub privilege_escalation_command: im::Vector<Arc<str>>,
}

enum ApplyGoal {
    SwitchToConfiguration(SwitchToConfigurationGoal),
    Push,
    Keys,
}

enum Goal {
    Apply {
        goal: ApplyGoal,
        should_apply_locally: bool,
        no_keys: bool,
        substitute_on_destination: bool,
        reboot: bool,
        host_platform: Arc<str>
    },
    Build,
}

#[enum_dispatch(ExecuteStep)]
enum Step {
    Ping,
    PushKeyAgent,
    Keys,
    Evaluate,
    PushEvaluatedOutput,
    Build,
    PushBuildOutput,
    SwitchToConfiguration,
}

struct NodePlan<'a> {
    context: Context<'a>,
    steps: Vec<Step>,
    node: Node,
}

fn create_plans<'a>(nodes: HashMap<Name, NodeRepr>, goal: &'_ Goal) -> Vec<NodePlan> {
    // let mut key_store = KeyStore::default();
    let mut plans = Vec::new();

    for (name, node) in nodes {
        let mut keys = Vec::with_capacity(node.keys.len());

        // for key in node.keys {
        //     let key = key_store.insert(key);
        //     keys.push(key);
        // }
        
        todo!()

        // let plan = plan_for_node(
        //     Node {
        //         target: node.target,
        //         build_remotely: node.build_remotely,
        //         allow_local_deployment: node.allow_local_deployment,
        //         tags: node.tags,
        //         host_platform: node.host_platform,
        //         privilege_escalation_command: node.privilege_escalation_command,
        //         keys,
        //     },
        //     name,
        //     goal,
        // );

        // plans.push(plan);
    }

    plans
}

fn plan_for_node<'a>(node: Node, name: Name, goal: &'_ Goal, hive_location: Arc<HiveLocation>, modifiers: SubCommandModifiers, should_quit: Arc<AtomicBool>) -> NodePlan {
    match goal {
        Goal::Build => NodePlan {
            context: Context {
                state: StepState::default(),
                modifiers,
                name,
                hive_location,
                should_quit
            },
            steps: vec![
                Step::Evaluate(Evaluate),
                Step::Build(Build { target: None }),
            ],
            node,
        },
        Goal::Apply {
            goal,
            should_apply_locally,
            no_keys,
            substitute_on_destination,
            reboot,
            host_platform
        } => {
            let mut steps: Vec<Step> = Vec::new();

            if !*should_apply_locally {
                steps.push(Step::Ping(Ping));
            }

            if !*no_keys
                && matches!(
                    &goal,
                    ApplyGoal::Keys
                        | ApplyGoal::SwitchToConfiguration(SwitchToConfigurationGoal::Switch)
                )
            {
                if !*should_apply_locally {
                    steps.push(Step::PushKeyAgent(PushKeyAgent {
                        substitute_on_destination: *substitute_on_destination,
                        host_platform,
                        target: node.target
                    }));
                }

                let keys = match goal {
                    ApplyGoal::SwitchToConfiguration(SwitchToConfigurationGoal::Switch) => node
                        .keys
                        .iter()
                        .filter(|x| matches!(x.upload_at, UploadKeyAt::PreActivation))
                        .cloned()
                        .collect(),
                    ApplyGoal::Keys => node.keys.clone(),
                    _ => unreachable!(),
                };

                if !keys.is_empty() {
                    steps.push(Step::Keys(Keys {
                        keys: node.keys.clone(),
                        target: if  *should_apply_locally {
                            Some(node.target)
                        } else {
                            None 
                        },
                        privilege_escalation_command: node.privilege_escalation_command.into()
                    }));
                }
            }

            steps.push(Step::Evaluate(Evaluate));

            if !matches!(goal, ApplyGoal::Keys)
                && !should_apply_locally
                && (node.build_remotely | matches!(goal, ApplyGoal::Push))
            {
                steps.push(Step::PushEvaluatedOutput(PushEvaluatedOutput {
                    substitute_on_destination: *substitute_on_destination,
                    target: node.target
                }));
            }

            if !matches!(goal, ApplyGoal::Keys | ApplyGoal::Push) {
                steps.push(Step::Build(Build {
                    target: if node.build_remotely && !*should_apply_locally  {
                        Some(node.target)
                    } else { None }
                }));
            }

            if !node.build_remotely
                && !should_apply_locally
                && !matches!(goal, ApplyGoal::Keys | ApplyGoal::Push)
            {
                steps.push(Step::PushBuildOutput(PushBuildOutput {
                    substitute_on_destination: *substitute_on_destination,
                    target: ctx.Target
                }));
            }

            if let ApplyGoal::SwitchToConfiguration(goal) = goal {
                steps.push(Step::SwitchToConfiguration(SwitchToConfiguration {
                    goal: *goal,
                    reboot: *reboot,
                    target: if *should_apply_locally {
                        Some(node.target)
                    } else { None },
                    privilege_escalation_command: node.privilege_escalation_command
                }));
            }

            NodePlan {
                context: Context {
                    state: StepState::default(),
                    name,
                    hive_location,
                    modifiers,
                    should_quit
                },
                steps,
                node,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // fn get_steps(goal_executor: GoalExecutor) -> std::vec::Vec<Step> {
    //     goal_executor
    //         .steps
    //         .into_iter()
    //         .filter(|step| step.should_execute(&goal_executor.context))
    //         .collect::<Vec<_>>()
    // }

    // #[tokio::test]
    // async fn order_build_locally() {
    //     // let location = location!(get_test_path!());
    //     // let mut node = Node {
    //     //     build_remotely: false,
    //     //     ..Default::default()
    //     // };
    //     // let name = &Name(function_name!().into());
    //     // let executor = GoalExecutor::new(Context::create_test_context(location, name, &mut node));
    //     // let steps = get_steps(executor);
    //
    //     let plan = plan_for_node(Node {
    //         build_remotely: false,
    //         ..Default::default()
    //     }, Name("".into()), &Goal::Build);
    //
    //     assert_eq!(
    //         plan.steps,
    //         vec![
    //             Step::Ping,
    //             Step::PushKeyAgent,
    //             Step::Keys,
    //             crate::hive::steps::evaluate::Evaluate.into(),
    //             crate::hive::steps::build::Build.into(),
    //             crate::hive::steps::push::PushBuildOutput.into(),
    //             SwitchToConfiguration.into(),
    //             Keys {
    //                 filter: UploadKeyAt::PostActivation
    //             }
    //             .into(),
    //         ]
    //     );
    // }

    // #[tokio::test]
    // async fn order_keys_only() {
    //     let location = location!(get_test_path!());
    //     let mut node = Node::default();
    //     let name = &Name(function_name!().into());
    //     let mut context = Context::create_test_context(location, name, &mut node);
    //
    //     let Objective::Apply(ref mut apply_objective) = context.objective else {
    //         unreachable!()
    //     };
    //
    //     apply_objective.goal = Goal::Keys;
    //
    //     let executor = GoalExecutor::new(context);
    //     let steps = get_steps(executor);
    //
    //     assert_eq!(
    //         steps,
    //         vec![
    //             Ping.into(),
    //             PushKeyAgent.into(),
    //             Keys {
    //                 filter: UploadKeyAt::NoFilter
    //             }
    //             .into(),
    //         ]
    //     );
    // }
    //
    // #[tokio::test]
    // async fn order_build() {
    //     let location = location!(get_test_path!());
    //     let mut node = Node::default();
    //     let name = &Name(function_name!().into());
    //     let mut context = Context::create_test_context(location, name, &mut node);
    //
    //     let Objective::Apply(ref mut apply_objective) = context.objective else {
    //         unreachable!()
    //     };
    //     apply_objective.goal = Goal::Build;
    //
    //     let executor = GoalExecutor::new(context);
    //     let steps = get_steps(executor);
    //
    //     assert_eq!(
    //         steps,
    //         vec![
    //             Ping.into(),
    //             crate::hive::steps::evaluate::Evaluate.into(),
    //             crate::hive::steps::build::Build.into(),
    //             crate::hive::steps::push::PushBuildOutput.into(),
    //         ]
    //     );
    // }
    //
    // #[tokio::test]
    // async fn order_push_only() {
    //     let location = location!(get_test_path!());
    //     let mut node = Node::default();
    //     let name = &Name(function_name!().into());
    //     let mut context = Context::create_test_context(location, name, &mut node);
    //
    //     let Objective::Apply(ref mut apply_objective) = context.objective else {
    //         unreachable!()
    //     };
    //     apply_objective.goal = Goal::Push;
    //
    //     let executor = GoalExecutor::new(context);
    //     let steps = get_steps(executor);
    //
    //     assert_eq!(
    //         steps,
    //         vec![
    //             Ping.into(),
    //             crate::hive::steps::evaluate::Evaluate.into(),
    //             crate::hive::steps::push::PushEvaluatedOutput.into(),
    //         ]
    //     );
    // }
    //
    // #[tokio::test]
    // async fn order_remote_build() {
    //     let location = location!(get_test_path!());
    //     let mut node = Node {
    //         build_remotely: true,
    //         ..Default::default()
    //     };
    //
    //     let name = &Name(function_name!().into());
    //     let executor = GoalExecutor::new(Context::create_test_context(location, name, &mut node));
    //     let steps = get_steps(executor);
    //
    //     assert_eq!(
    //         steps,
    //         vec![
    //             Ping.into(),
    //             PushKeyAgent.into(),
    //             Keys {
    //                 filter: UploadKeyAt::PreActivation
    //             }
    //             .into(),
    //             crate::hive::steps::evaluate::Evaluate.into(),
    //             crate::hive::steps::push::PushEvaluatedOutput.into(),
    //             crate::hive::steps::build::Build.into(),
    //             SwitchToConfiguration.into(),
    //             Keys {
    //                 filter: UploadKeyAt::PostActivation
    //             }
    //             .into(),
    //         ]
    //     );
    // }
    //
    // #[tokio::test]
    // async fn order_nokeys() {
    //     let location = location!(get_test_path!());
    //     let mut node = Node::default();
    //
    //     let name = &Name(function_name!().into());
    //     let mut context = Context::create_test_context(location, name, &mut node);
    //
    //     let Objective::Apply(ref mut apply_objective) = context.objective else {
    //         unreachable!()
    //     };
    //     apply_objective.no_keys = true;
    //
    //     let executor = GoalExecutor::new(context);
    //     let steps = get_steps(executor);
    //
    //     assert_eq!(
    //         steps,
    //         vec![
    //             Ping.into(),
    //             crate::hive::steps::evaluate::Evaluate.into(),
    //             crate::hive::steps::build::Build.into(),
    //             crate::hive::steps::push::PushBuildOutput.into(),
    //             SwitchToConfiguration.into(),
    //         ]
    //     );
    // }
    //
    // #[tokio::test]
    // async fn order_should_apply_locally() {
    //     let location = location!(get_test_path!());
    //     let mut node = Node::default();
    //
    //     let name = &Name(function_name!().into());
    //     let mut context = Context::create_test_context(location, name, &mut node);
    //
    //     let Objective::Apply(ref mut apply_objective) = context.objective else {
    //         unreachable!()
    //     };
    //     apply_objective.no_keys = true;
    //     apply_objective.should_apply_locally = true;
    //
    //     let executor = GoalExecutor::new(context);
    //     let steps = get_steps(executor);
    //
    //     assert_eq!(
    //         steps,
    //         vec![
    //             crate::hive::steps::evaluate::Evaluate.into(),
    //             crate::hive::steps::build::Build.into(),
    //             SwitchToConfiguration.into(),
    //         ]
    //     );
    // }
}
