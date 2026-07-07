use std::{
    collections::HashMap,
    sync::{Arc, atomic::AtomicBool, nonpoison::Mutex},
};

use tokio::sync::RwLock;

use crate::{
    SafeStorePath, SubCommandModifiers,
    hive::{
        HiveLocation,
        executor::{BuildOutputHandle, EvaluationOutputHandle, OutputHandle},
        node::{
            ApplyGoal, Context, HandleUnreachable, Name, Node, SharedTarget, Step, StepState,
            SwitchToConfigurationGoal,
        },
        steps::{
            activate::SwitchToConfiguration,
            build::{Build, BuildMetadata, NixCommandBuildMetadata},
            evaluate::Evaluate,
            keys::{Keys, PushKeyAgent, UploadKeyAt},
            ping::Ping,
            push::{PushBuildOutput, PushEvaluatedOutput},
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

fn apply_plan_keys(
    args: &ApplyGoalArgs,
    node: &Node,
    target: &SharedTarget,
) -> (Vec<Step>, Vec<Step>) {
    let ApplyGoalArgs {
        goal,
        substitute_on_destination,
        should_apply_locally,
        host_platform,
        ..
    } = args;
    let mut front_steps = Vec::new();
    let mut end_steps = Vec::new();
    let key_agent_directory = OutputHandle::new();

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

    // only push key agent if there are any keys at all
    if !pre_keys.is_empty() || !post_keys.is_empty() {
        front_steps.push(Step::PushKeyAgent(PushKeyAgent {
            substitute_on_destination: *substitute_on_destination,
            host_platform: host_platform.clone(),
            target: if *should_apply_locally {
                None
            } else {
                Some(target.clone())
            },
            key_agent_directory: key_agent_directory.clone(),
        }));
    }

    if !pre_keys.is_empty() {
        front_steps.push(Step::Keys(Keys {
            keys: pre_keys,
            target: if *should_apply_locally {
                None
            } else {
                Some(target.clone())
            },
            privilege_escalation_command: node.privilege_escalation_command.clone(),
            key_agent_directory: key_agent_directory.clone(),
        }));
    }

    if !post_keys.is_empty() {
        end_steps.push(Step::Keys(Keys {
            keys: post_keys,
            target: if *should_apply_locally {
                None
            } else {
                Some(target.clone())
            },
            privilege_escalation_command: node.privilege_escalation_command.clone(),
            key_agent_directory,
        }));
    }

    (front_steps, end_steps)
}

/// Push an `Evaluate` step onto `steps` when a real (non-cached) evaluation
/// is required, writing its result into `output`.
///
/// Returns `true` when the step was pushed. This is also exactly the
/// condition when callers should enable greedy evaluation.
fn push_evaluate_step(
    steps: &mut Vec<Step>,
    output: &EvaluationOutputHandle,
    needs_evaluate: bool,
    has_cached_evaluation: bool,
) -> bool {
    if needs_evaluate && !has_cached_evaluation {
        steps.push(Step::Evaluate(Evaluate {
            output: output.clone(),
        }));
        true
    } else {
        false
    }
}

#[allow(clippy::too_many_lines)]
fn apply_plan(
    args: &ApplyGoalArgs,
    node: &Node,
    name: &Name,
    modifiers: SubCommandModifiers,
    hive_location: Arc<HiveLocation>,
    should_quit: Arc<AtomicBool>,
    cached_evaluation: Option<&SafeStorePath<String>>,
) -> NodePlan {
    let ApplyGoalArgs {
        goal,
        should_apply_locally,
        no_keys,
        substitute_on_destination,
        reboot,
        handle_unreachable,
        ..
    } = args;

    let mut steps: Vec<Step> = Vec::new();
    let mut end: Vec<Step> = Vec::new();
    let target = SharedTarget(Arc::new(RwLock::new(node.target.clone())));
    let build_will_have_target = node.build_remotely && !*should_apply_locally;

    let evaluation_output_handle = cached_evaluation
        .map_or_else(EvaluationOutputHandle::new, |cached_evaluation| {
            EvaluationOutputHandle::new_known(cached_evaluation.clone())
        });

    let build_output_handle = BuildOutputHandle::new();

    if !*should_apply_locally {
        steps.push(Step::Ping(Ping {
            target: target.clone(),
        }));
    }

    let needs_separate_eval = match goal {
        ApplyGoal::Keys => false,
        // a `.drv` must be known before so it can be pushed
        ApplyGoal::Push => true,
        // the experimental nix client still requires the path to build
        _ if modifiers.experimental_nix_client => true,
        // only evaluate if the build step will require a real `.drv` on the remote system
        // or if it can use an attribute that exists on the local host directly
        _ => build_will_have_target,
    };
    let has_cached_evaluation = cached_evaluation.is_some();

    let greedy_evaluate = push_evaluate_step(
        &mut steps,
        &evaluation_output_handle,
        needs_separate_eval,
        has_cached_evaluation,
    );

    if !matches!(goal, ApplyGoal::Keys)
        && !should_apply_locally
        && (node.build_remotely || matches!(goal, ApplyGoal::Push))
    {
        steps.push(Step::PushEvaluatedOutput(PushEvaluatedOutput {
            substitute_on_destination: *substitute_on_destination,
            target: target.clone(),
            path: evaluation_output_handle.clone(),
        }));
    }

    if !matches!(goal, ApplyGoal::Keys | ApplyGoal::Push) {
        steps.push(Step::Build(Build {
            output: build_output_handle.clone(),
            metadata: if modifiers.experimental_nix_client {
                BuildMetadata::BuildWithNixDaemon {
                    target: if build_will_have_target {
                        Some(target.clone())
                    } else {
                        None
                    },
                    derivation: evaluation_output_handle,
                }
            } else {
                BuildMetadata::NixCommand(if build_will_have_target {
                    NixCommandBuildMetadata::Remotely {
                        target: target.clone(),
                        derivation: evaluation_output_handle,
                    }
                } else {
                    NixCommandBuildMetadata::Locally {
                        cached_derivation: if cached_evaluation.is_some() {
                            Some(evaluation_output_handle)
                        } else {
                            None
                        },
                    }
                })
            },
        }));
    }

    if !node.build_remotely
        && !should_apply_locally
        && !matches!(goal, ApplyGoal::Keys | ApplyGoal::Push)
    {
        steps.push(Step::PushBuildOutput(PushBuildOutput {
            substitute_on_destination: *substitute_on_destination,
            target: target.clone(),
            path: build_output_handle.clone(),
        }));
    }

    if !*no_keys
        && matches!(
            &goal,
            ApplyGoal::Keys | ApplyGoal::SwitchToConfiguration(SwitchToConfigurationGoal::Switch)
        )
    {
        let (pre, post) = apply_plan_keys(args, node, &target);
        steps.extend(pre);
        end.extend(post);
    }

    if let ApplyGoal::SwitchToConfiguration(goal) = goal {
        steps.push(Step::SwitchToConfiguration(SwitchToConfiguration {
            goal: *goal,
            reboot: *reboot,
            target: if *should_apply_locally {
                None
            } else {
                Some(target)
            },
            privilege_escalation_command: node.privilege_escalation_command.clone(),
            top_level: build_output_handle,
        }));
    }

    steps.extend(end);

    NodePlan {
        context: Context {
            state: StepState::default(),
            name: name.clone(),
            hive_location,
            modifiers,
            should_quit,
            build_id_names: Arc::new(Mutex::new(HashMap::new())),
        },
        steps,
        greedy_evaluate,
        ignore_failed_ping: matches!(handle_unreachable, HandleUnreachable::Ignore),
    }
}

pub fn plan_for_node(
    node: &Node,
    name: Name,
    goal: &'_ Goal,
    hive_location: Arc<HiveLocation>,
    modifiers: &SubCommandModifiers,
    should_quit: Arc<AtomicBool>,
    cached_evaluation: Option<&SafeStorePath<String>>,
) -> NodePlan {
    match goal {
        Goal::Build => {
            let evaluation_output_handle = cached_evaluation
                .map_or_else(EvaluationOutputHandle::new, |cached_evaluation| {
                    EvaluationOutputHandle::new_known(cached_evaluation.clone())
                });

            let mut steps = Vec::new();

            let greedy_evaluate = push_evaluate_step(
                &mut steps,
                &evaluation_output_handle,
                modifiers.experimental_nix_client,
                cached_evaluation.is_some(),
            );

            steps.push(Step::Build(Build {
                output: BuildOutputHandle::new(),
                metadata: if modifiers.experimental_nix_client {
                    BuildMetadata::BuildWithNixDaemon {
                        target: None,
                        derivation: evaluation_output_handle,
                    }
                } else {
                    BuildMetadata::NixCommand(NixCommandBuildMetadata::Locally {
                        cached_derivation: if cached_evaluation.is_some() {
                            Some(evaluation_output_handle)
                        } else {
                            None
                        },
                    })
                },
            }));

            NodePlan {
                context: Context {
                    state: StepState::default(),
                    modifiers: *modifiers,
                    hive_location,
                    should_quit,
                    name,
                    build_id_names: Arc::new(Mutex::new(HashMap::new())),
                },
                steps,
                greedy_evaluate,
                ignore_failed_ping: false,
            }
        }
        Goal::Apply(args) => apply_plan(
            args,
            node,
            &name,
            *modifiers,
            hive_location,
            should_quit,
            cached_evaluation,
        ),
    }
}

#[cfg(test)]
mod tests {
    use tokio::sync::RwLock;

    use crate::{
        SafeStorePath, SubCommandModifiers, function_name, get_test_path,
        hive::{
            executor::{BuildOutputHandle, EvaluationOutputHandle, OutputHandle},
            node::{
                ApplyGoal, HandleUnreachable, Name, Node, SharedTarget, Step,
                SwitchToConfigurationGoal,
            },
            plan::{ApplyGoalArgs, Goal, plan_for_node},
            steps::{
                activate::SwitchToConfiguration,
                build::{Build, BuildMetadata, NixCommandBuildMetadata},
                evaluate::Evaluate,
                keys::{Key, Keys, PushKeyAgent, Source, UploadKeyAt},
                ping::Ping,
                push::{PushBuildOutput, PushEvaluatedOutput},
            },
        },
        location,
    };
    use std::path::PathBuf;
    use std::{
        env,
        sync::{Arc, atomic::AtomicBool},
    };

    fn new_key(upload_at: &UploadKeyAt) -> Key {
        Key {
            upload_at: upload_at.clone(),
            source: Source::String(match upload_at {
                UploadKeyAt::PreActivation => "pre".into(),
                UploadKeyAt::PostActivation => "post".into(),
                UploadKeyAt::NoFilter => "none".into(),
            }),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn order_build() {
        let location = location!(get_test_path!());
        let node = Node {
            build_remotely: false,
            ..Default::default()
        };
        let name = &Name(function_name!().into());
        let should_quit = Arc::new(AtomicBool::new(false));
        let plan = plan_for_node(
            &node,
            name.clone(),
            &Goal::Build,
            location.into(),
            &SubCommandModifiers::default(),
            should_quit,
            None,
        );

        assert_eq!(
            plan.steps,
            vec![
                Build {
                    output: BuildOutputHandle::new(),
                    metadata: BuildMetadata::NixCommand(NixCommandBuildMetadata::Locally {
                        cached_derivation: None,
                    }),
                }
                .into()
            ]
        );
        assert!(!plan.greedy_evaluate);
    }

    #[tokio::test]
    async fn order_apply_build() {
        let location = location!(get_test_path!());
        let node = Node {
            build_remotely: true,
            ..Default::default()
        };
        let name = &Name(function_name!().into());
        let should_quit = Arc::new(AtomicBool::new(false));
        let target = SharedTarget(Arc::new(RwLock::new(node.target.clone())));
        let plan = plan_for_node(
            &node,
            name.clone(),
            &Goal::Apply(ApplyGoalArgs {
                goal: ApplyGoal::Build,
                should_apply_locally: false,
                no_keys: true,
                substitute_on_destination: true,
                reboot: false,
                host_platform: "x86_64-linux".into(),
                handle_unreachable: HandleUnreachable::default(),
            }),
            location.clone().into(),
            &SubCommandModifiers::default(),
            should_quit.clone(),
            None,
        );

        assert_eq!(
            plan.steps,
            vec![
                Ping {
                    target: target.clone()
                }
                .into(),
                Evaluate {
                    output: EvaluationOutputHandle::new(),
                }
                .into(),
                PushEvaluatedOutput {
                    substitute_on_destination: true,
                    target: target.clone(),
                    path: EvaluationOutputHandle::new(),
                }
                .into(),
                Build {
                    output: BuildOutputHandle::new(),
                    metadata: BuildMetadata::NixCommand(NixCommandBuildMetadata::Remotely {
                        target: target.clone(),
                        derivation: EvaluationOutputHandle::new(),
                    }),
                }
                .into(),
            ]
        );
        assert!(plan.greedy_evaluate);

        let node = Node {
            build_remotely: false,
            ..Default::default()
        };
        let plan = plan_for_node(
            &node,
            name.clone(),
            &Goal::Apply(ApplyGoalArgs {
                goal: ApplyGoal::Build,
                should_apply_locally: false,
                no_keys: true,
                substitute_on_destination: true,
                reboot: false,
                host_platform: "x86_64-linux".into(),
                handle_unreachable: HandleUnreachable::default(),
            }),
            location.into(),
            &SubCommandModifiers::default(),
            should_quit,
            None,
        );

        assert_eq!(
            plan.steps,
            vec![
                Ping {
                    target: target.clone()
                }
                .into(),
                Build {
                    output: BuildOutputHandle::new(),
                    metadata: BuildMetadata::NixCommand(NixCommandBuildMetadata::Locally {
                        cached_derivation: None,
                    }),
                }
                .into(),
                PushBuildOutput {
                    substitute_on_destination: true,
                    target,
                    path: BuildOutputHandle::new(),
                }
                .into(),
            ]
        );
        assert!(!plan.greedy_evaluate);
    }

    #[tokio::test]
    async fn order_keys_only() {
        let location = location!(get_test_path!());
        let node = Node {
            keys: vec![
                new_key(&UploadKeyAt::PreActivation).into(),
                new_key(&UploadKeyAt::PostActivation).into(),
                new_key(&UploadKeyAt::PreActivation).into(),
                new_key(&UploadKeyAt::PostActivation).into(),
            ],
            ..Default::default()
        };
        let name = &Name(function_name!().into());
        let should_quit = Arc::new(AtomicBool::new(false));
        let target = SharedTarget(Arc::new(RwLock::new(node.target.clone())));
        let plan_apply_keys = plan_for_node(
            &node,
            name.clone(),
            &Goal::Apply(ApplyGoalArgs {
                goal: ApplyGoal::Keys,
                should_apply_locally: false,
                no_keys: false,
                substitute_on_destination: true,
                reboot: false,
                host_platform: "x86_64-linux".into(),
                handle_unreachable: HandleUnreachable::default(),
            }),
            location.into(),
            &SubCommandModifiers::default(),
            should_quit,
            None,
        );

        assert_eq!(
            plan_apply_keys.steps,
            vec![
                Ping {
                    target: target.clone()
                }
                .into(),
                PushKeyAgent {
                    substitute_on_destination: true,
                    target: Some(target.clone()),
                    host_platform: node.host_platform.clone(),
                    key_agent_directory: OutputHandle::new(),
                }
                .into(),
                Keys {
                    target: Some(target),
                    // test that all keys are included
                    keys: node.keys.clone(),
                    privilege_escalation_command: node.privilege_escalation_command,
                    key_agent_directory: OutputHandle::new(),
                }
                .into(),
            ]
        );
        assert!(!plan_apply_keys.greedy_evaluate);
    }

    #[tokio::test]
    async fn order_key_split() {
        let location = location!(get_test_path!());
        let node = Node {
            keys: vec![
                new_key(&UploadKeyAt::PreActivation).into(),
                new_key(&UploadKeyAt::PostActivation).into(),
                new_key(&UploadKeyAt::PreActivation).into(),
                new_key(&UploadKeyAt::PostActivation).into(),
            ],
            ..Default::default()
        };
        let name = &Name(function_name!().into());
        let should_quit = Arc::new(AtomicBool::new(false));

        // Test that keys are split by their `upload_at`, also tests that key
        // step's `target` abides by should_apply_locally
        let plan_activate_with_keys = plan_for_node(
            &node,
            name.clone(),
            &Goal::Apply(ApplyGoalArgs {
                goal: ApplyGoal::SwitchToConfiguration(
                    crate::hive::node::SwitchToConfigurationGoal::Switch,
                ),
                should_apply_locally: true,
                no_keys: false,
                substitute_on_destination: true,
                reboot: false,
                host_platform: "x86_64-linux".into(),
                handle_unreachable: HandleUnreachable::default(),
            }),
            location.into(),
            &SubCommandModifiers::default(),
            should_quit,
            None,
        );

        assert_eq!(
            plan_activate_with_keys
                .steps
                .into_iter()
                .filter(|x| matches!(
                    x,
                    Step::Keys(Keys { .. }) | Step::PushKeyAgent(PushKeyAgent { .. })
                ))
                .collect::<Vec<Step>>(),
            vec![
                PushKeyAgent {
                    substitute_on_destination: true,
                    target: None,
                    host_platform: node.host_platform.clone(),
                    key_agent_directory: OutputHandle::new(),
                }
                .into(),
                Keys {
                    target: None,
                    keys: node
                        .keys
                        .iter()
                        .filter(|key| matches!(key.upload_at, UploadKeyAt::PreActivation))
                        .cloned()
                        .collect::<Vec<_>>(),
                    privilege_escalation_command: node.privilege_escalation_command.clone(),
                    key_agent_directory: OutputHandle::new(),
                }
                .into(),
                Keys {
                    target: None,
                    keys: node
                        .keys
                        .iter()
                        .filter(|key| matches!(key.upload_at, UploadKeyAt::PostActivation))
                        .cloned()
                        .collect::<Vec<_>>(),
                    privilege_escalation_command: node.privilege_escalation_command.clone(),
                    key_agent_directory: OutputHandle::new(),
                }
                .into(),
            ]
        );
    }

    #[tokio::test]
    async fn order_push_only() {
        let location = location!(get_test_path!());
        let node = Node::default();
        let name = &Name(function_name!().into());
        let should_quit = Arc::new(AtomicBool::new(false));
        let target = SharedTarget(Arc::new(RwLock::new(node.target.clone())));
        let plan = plan_for_node(
            &node,
            name.clone(),
            &Goal::Apply(ApplyGoalArgs {
                goal: ApplyGoal::Push,
                should_apply_locally: false,
                no_keys: false,
                substitute_on_destination: true,
                reboot: false,
                host_platform: "x86_64-linux".into(),
                handle_unreachable: HandleUnreachable::default(),
            }),
            location.into(),
            &SubCommandModifiers::default(),
            should_quit,
            None,
        );

        assert_eq!(
            plan.steps,
            vec![
                Ping {
                    target: target.clone()
                }
                .into(),
                Evaluate {
                    output: EvaluationOutputHandle::new(),
                }
                .into(),
                PushEvaluatedOutput {
                    substitute_on_destination: true,
                    target,
                    path: EvaluationOutputHandle::new(),
                }
                .into()
            ]
        );
        assert!(plan.greedy_evaluate);
    }

    #[tokio::test]
    async fn order_remote_build() {
        let location = location!(get_test_path!());
        let node = Node {
            build_remotely: true,
            ..Default::default()
        };
        let name = &Name(function_name!().into());
        let should_quit = Arc::new(AtomicBool::new(false));
        let target = SharedTarget(Arc::new(RwLock::new(node.target.clone())));
        let plan = plan_for_node(
            &node,
            name.clone(),
            &Goal::Apply(ApplyGoalArgs {
                goal: ApplyGoal::SwitchToConfiguration(SwitchToConfigurationGoal::Switch),
                should_apply_locally: false,
                no_keys: false,
                substitute_on_destination: true,
                reboot: false,
                host_platform: "x86_64-linux".into(),
                handle_unreachable: HandleUnreachable::default(),
            }),
            location.into(),
            &SubCommandModifiers::default(),
            should_quit,
            None,
        );

        assert_eq!(
            plan.steps,
            vec![
                Ping {
                    target: target.clone()
                }
                .into(),
                Evaluate {
                    output: EvaluationOutputHandle::new(),
                }
                .into(),
                PushEvaluatedOutput {
                    substitute_on_destination: true,
                    target: target.clone(),
                    path: EvaluationOutputHandle::new(),
                }
                .into(),
                Build {
                    output: BuildOutputHandle::new(),
                    metadata: BuildMetadata::NixCommand(NixCommandBuildMetadata::Remotely {
                        target: target.clone(),
                        derivation: EvaluationOutputHandle::new(),
                    }),
                }
                .into(),
                SwitchToConfiguration {
                    goal: SwitchToConfigurationGoal::Switch,
                    reboot: false,
                    target: Some(target),
                    privilege_escalation_command: node.privilege_escalation_command,
                    top_level: BuildOutputHandle::new(),
                }
                .into(),
            ]
        );
        assert!(plan.greedy_evaluate);
    }

    #[tokio::test]
    async fn order_nokeys() {
        let location = location!(get_test_path!());
        let node = Node {
            keys: vec![Key::default().into(), Key::default().into()],
            build_remotely: true,
            ..Default::default()
        };
        let name = &Name(function_name!().into());
        let should_quit = Arc::new(AtomicBool::new(false));
        let target = SharedTarget(Arc::new(RwLock::new(node.target.clone())));
        let plan = plan_for_node(
            &node,
            name.clone(),
            &Goal::Apply(ApplyGoalArgs {
                goal: ApplyGoal::SwitchToConfiguration(SwitchToConfigurationGoal::Switch),
                should_apply_locally: false,
                no_keys: true,
                substitute_on_destination: true,
                reboot: false,
                host_platform: "x86_64-linux".into(),
                handle_unreachable: HandleUnreachable::default(),
            }),
            location.into(),
            &SubCommandModifiers::default(),
            should_quit,
            None,
        );

        assert_eq!(
            plan.steps,
            vec![
                Ping {
                    target: target.clone()
                }
                .into(),
                Evaluate {
                    output: EvaluationOutputHandle::new(),
                }
                .into(),
                PushEvaluatedOutput {
                    substitute_on_destination: true,
                    target: target.clone(),
                    path: EvaluationOutputHandle::new(),
                }
                .into(),
                Build {
                    output: BuildOutputHandle::new(),
                    metadata: BuildMetadata::NixCommand(NixCommandBuildMetadata::Remotely {
                        target: target.clone(),
                        derivation: EvaluationOutputHandle::new(),
                    }),
                }
                .into(),
                SwitchToConfiguration {
                    goal: SwitchToConfigurationGoal::Switch,
                    reboot: false,
                    target: Some(target),
                    privilege_escalation_command: node.privilege_escalation_command,
                    top_level: BuildOutputHandle::new(),
                }
                .into(),
            ]
        );
        assert!(plan.greedy_evaluate);
    }

    #[tokio::test]
    async fn order_should_apply_locally() {
        let location = location!(get_test_path!());
        let node = Node::default();
        let name = &Name(function_name!().into());
        let should_quit = Arc::new(AtomicBool::new(false));
        let plan = plan_for_node(
            &node,
            name.clone(),
            &Goal::Apply(ApplyGoalArgs {
                goal: ApplyGoal::SwitchToConfiguration(SwitchToConfigurationGoal::Switch),
                should_apply_locally: true,
                no_keys: true,
                substitute_on_destination: true,
                reboot: false,
                host_platform: "x86_64-linux".into(),
                handle_unreachable: HandleUnreachable::default(),
            }),
            location.into(),
            &SubCommandModifiers::default(),
            should_quit,
            None,
        );

        assert_eq!(
            plan.steps,
            vec![
                Build {
                    output: BuildOutputHandle::new(),
                    metadata: BuildMetadata::NixCommand(NixCommandBuildMetadata::Locally {
                        cached_derivation: None,
                    }),
                }
                .into(),
                SwitchToConfiguration {
                    goal: SwitchToConfigurationGoal::Switch,
                    reboot: false,
                    target: None,
                    privilege_escalation_command: node.privilege_escalation_command,
                    top_level: BuildOutputHandle::new(),
                }
                .into(),
            ]
        );
        assert!(!plan.greedy_evaluate);
    }

    #[tokio::test]
    async fn order_build_cached_evaluation() {
        let location = location!(get_test_path!());
        let node = Node::default();
        let name = &Name(function_name!().into());
        let should_quit = Arc::new(AtomicBool::new(false));
        let cached = SafeStorePath::<String>::from_absolute_path(b"/nix/store/name").unwrap();
        let plan = plan_for_node(
            &node,
            name.clone(),
            &Goal::Build,
            location.into(),
            &SubCommandModifiers::default(),
            should_quit,
            Some(&cached),
        );

        // a cached evaluation means no Evaluate step is scheduled, and the
        // local build reuses the known derivation path.
        assert_eq!(
            plan.steps,
            vec![
                Build {
                    output: BuildOutputHandle::new(),
                    metadata: BuildMetadata::NixCommand(NixCommandBuildMetadata::Locally {
                        cached_derivation: Some(EvaluationOutputHandle::new_known(cached)),
                    }),
                }
                .into()
            ]
        );
        assert!(!plan.greedy_evaluate);
    }

    #[tokio::test]
    async fn order_build_experimental_nix_client() {
        let location = location!(get_test_path!());
        let node = Node::default();
        let name = &Name(function_name!().into());
        let should_quit = Arc::new(AtomicBool::new(false));
        let plan = plan_for_node(
            &node,
            name.clone(),
            &Goal::Build,
            location.into(),
            &SubCommandModifiers {
                experimental_nix_client: true,
                ..Default::default()
            },
            should_quit,
            None,
        );

        // the experimental nix client needs a real `.drv`, so an Evaluate
        // step is scheduled even for a local Build.
        assert_eq!(
            plan.steps,
            vec![
                Evaluate {
                    output: EvaluationOutputHandle::new(),
                }
                .into(),
                Build {
                    output: BuildOutputHandle::new(),
                    metadata: BuildMetadata::BuildWithNixDaemon {
                        target: None,
                        derivation: EvaluationOutputHandle::new(),
                    },
                }
                .into(),
            ]
        );
        assert!(plan.greedy_evaluate);
    }

    #[tokio::test]
    async fn order_remote_build_experimental_nix_client() {
        let location = location!(get_test_path!());
        let node = Node {
            build_remotely: true,
            ..Default::default()
        };
        let name = &Name(function_name!().into());
        let should_quit = Arc::new(AtomicBool::new(false));
        let target = SharedTarget(Arc::new(RwLock::new(node.target.clone())));
        let plan = plan_for_node(
            &node,
            name.clone(),
            &Goal::Apply(ApplyGoalArgs {
                goal: ApplyGoal::SwitchToConfiguration(SwitchToConfigurationGoal::Switch),
                should_apply_locally: false,
                no_keys: true,
                substitute_on_destination: true,
                reboot: false,
                host_platform: "x86_64-linux".into(),
                handle_unreachable: HandleUnreachable::default(),
            }),
            location.into(),
            &SubCommandModifiers {
                experimental_nix_client: true,
                ..Default::default()
            },
            should_quit,
            None,
        );

        assert_eq!(
            plan.steps,
            vec![
                Ping {
                    target: target.clone()
                }
                .into(),
                Evaluate {
                    output: EvaluationOutputHandle::new(),
                }
                .into(),
                PushEvaluatedOutput {
                    substitute_on_destination: true,
                    target: target.clone(),
                    path: EvaluationOutputHandle::new(),
                }
                .into(),
                Build {
                    output: BuildOutputHandle::new(),
                    metadata: BuildMetadata::BuildWithNixDaemon {
                        target: Some(target.clone()),
                        derivation: EvaluationOutputHandle::new(),
                    },
                }
                .into(),
                SwitchToConfiguration {
                    goal: SwitchToConfigurationGoal::Switch,
                    reboot: false,
                    target: Some(target),
                    privilege_escalation_command: node.privilege_escalation_command,
                    top_level: BuildOutputHandle::new(),
                }
                .into(),
            ]
        );
        assert!(plan.greedy_evaluate);
    }
}
