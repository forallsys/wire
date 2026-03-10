use std::sync::{Arc, atomic::AtomicBool};

use crate::{SubCommandModifiers, hive::{HiveLocation, node::{ApplyGoal, Context, HandleUnreachable, Name, Node, Step, StepState, SwitchToConfigurationGoal}, steps::{activate::SwitchToConfiguration, build::Build, evaluate::Evaluate, keys::{Keys, PushKeyAgent, UploadKeyAt}, ping::Ping, push::{PushBuildOutput, PushEvaluatedOutput}}}};

pub struct NodePlan {
    pub context: Context,
    pub steps: Vec<Step>,
    pub greedy_evaluate: bool,
    pub ignore_failed_ping: bool
}

pub enum Goal {
    Apply {
        goal: ApplyGoal,
        should_apply_locally: bool,
        no_keys: bool,
        substitute_on_destination: bool,
        reboot: bool,
        host_platform: Arc<str>,
        handle_unreachable: HandleUnreachable
    },
    Build,
}

// TODO: remove this allow
#[allow(clippy::too_many_lines)]
pub fn plan_for_node(node: &Node, name: Name, goal: &'_ Goal, hive_location: Arc<HiveLocation>, modifiers: &SubCommandModifiers, should_quit: Arc<AtomicBool>) -> NodePlan {
    match goal {
        Goal::Build => NodePlan {
            context: Context {
                state: StepState::default(),
                modifiers: *modifiers,
                hive_location,
                should_quit,
                name
            },
            steps: vec![
                Step::Evaluate(Evaluate),
                Step::Build(Build { target: None }),
            ],
            greedy_evaluate: true,
            ignore_failed_ping: false
        },
        Goal::Apply {
            goal,
            should_apply_locally,
            no_keys,
            substitute_on_destination,
            reboot,
            host_platform,
            handle_unreachable
        } => {
            let mut steps: Vec<Step> = Vec::new();

            if !*should_apply_locally {
                steps.push(Step::Ping(Ping { }));
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
                        host_platform: host_platform.clone(),
                        target: node.target.clone()
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
                            Some(node.target.clone())
                        } else {
                            None 
                        },
                        privilege_escalation_command: node.privilege_escalation_command.clone()
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
                    target: node.target.clone()
                }));
            }

            if !matches!(goal, ApplyGoal::Keys | ApplyGoal::Push) {
                steps.push(Step::Build(Build {
                    target: if node.build_remotely && !*should_apply_locally  {
                        Some(node.target.clone())
                    } else { None }
                }));
            }

            if !node.build_remotely
                && !should_apply_locally
                && !matches!(goal, ApplyGoal::Keys | ApplyGoal::Push)
            {
                steps.push(Step::PushBuildOutput(PushBuildOutput {
                    substitute_on_destination: *substitute_on_destination,
                    target: node.target.clone()
                }));
            }

            if let ApplyGoal::SwitchToConfiguration(goal) = goal {
                steps.push(Step::SwitchToConfiguration(SwitchToConfiguration {
                    goal: *goal,
                    reboot: *reboot,
                    target: if *should_apply_locally {
                        Some(node.target.clone())
                    } else { None },
                    privilege_escalation_command: node.privilege_escalation_command.clone()
                }));
            }

            NodePlan {
                context: Context {
                    state: StepState::default(),
                    name: name.clone(),
                    hive_location,
                    modifiers: *modifiers,
                    should_quit
                },
                steps,
                greedy_evaluate: !matches!(
                    &goal,
                    ApplyGoal::Keys
                ),
                ignore_failed_ping: matches!(handle_unreachable, HandleUnreachable::Ignore)
            }
        }
    }
}
