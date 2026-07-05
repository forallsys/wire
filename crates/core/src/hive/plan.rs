use std::{
    collections::HashMap,
    sync::{Arc, atomic::AtomicBool, nonpoison::Mutex},
};

use tokio::sync::RwLock;

use crate::{
    SafeStorePath, SubCommandModifiers,
    hive::{
        HiveLocation,
        node::{
            ApplyGoal, Context, HandleUnreachable, Name, Node, SharedTarget, Step, StepState,
            SwitchToConfigurationGoal,
        },
        steps::{
            activate::SwitchToConfiguration,
            build::Build,
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
        }));
    }

    (front_steps, end_steps)
}

fn apply_plan(
    args: &ApplyGoalArgs,
    node: &Node,
    name: &Name,
    modifiers: SubCommandModifiers,
    hive_location: Arc<HiveLocation>,
    should_quit: Arc<AtomicBool>,
    cached_evaluation: Option<SafeStorePath<String>>,
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
    let has_cached_evaluation = cached_evaluation.is_some();

    if !*should_apply_locally {
        steps.push(Step::Ping(Ping {
            target: target.clone(),
        }));
    }

    if !matches!(goal, ApplyGoal::Keys) {
        steps.push(Step::Evaluate(Evaluate { cached_evaluation }));
    }

    if !matches!(goal, ApplyGoal::Keys)
        && !should_apply_locally
        && (node.build_remotely || matches!(goal, ApplyGoal::Push))
    {
        steps.push(Step::PushEvaluatedOutput(PushEvaluatedOutput {
            substitute_on_destination: *substitute_on_destination,
            target: target.clone(),
        }));
    }

    if !matches!(goal, ApplyGoal::Keys | ApplyGoal::Push) {
        steps.push(Step::Build(Build {
            target: if node.build_remotely && !*should_apply_locally {
                Some(target.clone())
            } else {
                None
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
        greedy_evaluate: !matches!(&goal, ApplyGoal::Keys) && !has_cached_evaluation,
        ignore_failed_ping: matches!(handle_unreachable, HandleUnreachable::Ignore),
    }
}

#[allow(clippy::too_many_lines)]
pub fn plan_for_node(
    node: &Node,
    name: Name,
    goal: &'_ Goal,
    hive_location: Arc<HiveLocation>,
    modifiers: &SubCommandModifiers,
    should_quit: Arc<AtomicBool>,
    cached_evaluation: Option<SafeStorePath<String>>,
) -> NodePlan {
    let greedy_evaluate = cached_evaluation.is_none();

    match goal {
        Goal::Build => NodePlan {
            context: Context {
                state: StepState::default(),
                modifiers: *modifiers,
                hive_location,
                should_quit,
                name,
                build_id_names: Arc::new(Mutex::new(HashMap::new())),
            },
            steps: vec![
                Step::Evaluate(Evaluate { cached_evaluation }),
                Step::Build(Build { target: None }),
            ],
            greedy_evaluate,
            ignore_failed_ping: false,
        },
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
        SubCommandModifiers, function_name, get_test_path,
        hive::{
            node::{
                ApplyGoal, HandleUnreachable, Name, Node, SharedTarget, Step,
                SwitchToConfigurationGoal,
            },
            plan::{ApplyGoalArgs, Goal, plan_for_node},
            steps::{
                activate::SwitchToConfiguration,
                build::Build,
                evaluate::Evaluate,
                keys::{Key, Keys, PushKeyAgent, Source, UploadKeyAt},
                ping::Ping,
                push::PushEvaluatedOutput,
            },
        },
        location,
    };
    use quickcheck::{Arbitrary, Gen};
    use quickcheck_macros::quickcheck;
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
                Evaluate {
                    cached_evaluation: None
                }
                .into(),
                Build { target: None }.into()
            ]
        );
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
                crate::hive::steps::evaluate::Evaluate {
                    cached_evaluation: None
                }
                .into(),
                crate::hive::steps::push::PushEvaluatedOutput {
                    substitute_on_destination: true,
                    target: target.clone()
                }
                .into(),
                crate::hive::steps::build::Build {
                    target: Some(target.clone())
                }
                .into(),
            ]
        );

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
                crate::hive::steps::evaluate::Evaluate {
                    cached_evaluation: None
                }
                .into(),
                crate::hive::steps::build::Build { target: None }.into(),
                crate::hive::steps::push::PushBuildOutput {
                    substitute_on_destination: true,
                    target
                }
                .into(),
            ]
        );
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
                    host_platform: node.host_platform.clone()
                }
                .into(),
                Keys {
                    target: Some(target),
                    // test that all keys are included
                    keys: node.keys.clone(),
                    privilege_escalation_command: node.privilege_escalation_command
                }
                .into(),
            ]
        );
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
                    host_platform: node.host_platform.clone()
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
                    privilege_escalation_command: node.privilege_escalation_command.clone()
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
                    privilege_escalation_command: node.privilege_escalation_command.clone()
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
                    cached_evaluation: None
                }
                .into(),
                PushEvaluatedOutput {
                    substitute_on_destination: true,
                    target
                }
                .into()
            ]
        );
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
                    cached_evaluation: None
                }
                .into(),
                PushEvaluatedOutput {
                    substitute_on_destination: true,
                    target: target.clone()
                }
                .into(),
                Build {
                    target: Some(target.clone())
                }
                .into(),
                SwitchToConfiguration {
                    goal: SwitchToConfigurationGoal::Switch,
                    reboot: false,
                    target: Some(target),
                    privilege_escalation_command: node.privilege_escalation_command,
                }
                .into(),
            ]
        );
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
                    cached_evaluation: None
                }
                .into(),
                PushEvaluatedOutput {
                    substitute_on_destination: true,
                    target: target.clone()
                }
                .into(),
                Build {
                    target: Some(target.clone())
                }
                .into(),
                SwitchToConfiguration {
                    goal: SwitchToConfigurationGoal::Switch,
                    reboot: false,
                    target: Some(target),
                    privilege_escalation_command: node.privilege_escalation_command,
                }
                .into(),
            ]
        );
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
                Evaluate {
                    cached_evaluation: None
                }
                .into(),
                Build { target: None }.into(),
                SwitchToConfiguration {
                    goal: SwitchToConfigurationGoal::Switch,
                    reboot: false,
                    target: None,
                    privilege_escalation_command: node.privilege_escalation_command,
                }
                .into(),
            ]
        );
    }
}
