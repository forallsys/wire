use std::sync::{Arc, atomic::AtomicBool};

use crate::{
    SubCommandModifiers,
    hive::{
        HiveLocation,
        node::{
            ApplyGoal, Context, HandleUnreachable, Name, Node, Step, StepState,
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

pub enum Goal {
    Apply {
        goal: ApplyGoal,
        should_apply_locally: bool,
        no_keys: bool,
        substitute_on_destination: bool,
        reboot: bool,
        host_platform: Arc<str>,
        handle_unreachable: HandleUnreachable,
    },
    Build,
}

// TODO: remove this allow
#[allow(clippy::too_many_lines)]
pub fn plan_for_node(
    node: &Node,
    name: Name,
    goal: &'_ Goal,
    hive_location: Arc<HiveLocation>,
    modifiers: &SubCommandModifiers,
    should_quit: Arc<AtomicBool>,
) -> NodePlan {
    match goal {
        Goal::Build => NodePlan {
            context: Context {
                state: StepState::default(),
                modifiers: *modifiers,
                hive_location,
                should_quit,
                name,
            },
            steps: vec![
                Step::Evaluate(Evaluate),
                Step::Build(Build { target: None }),
            ],
            greedy_evaluate: true,
            ignore_failed_ping: false,
        },
        Goal::Apply {
            goal,
            should_apply_locally,
            no_keys,
            substitute_on_destination,
            reboot,
            host_platform,
            handle_unreachable,
        } => {
            let mut steps: Vec<Step> = Vec::new();
            let mut end: Vec<Step> = Vec::new();

            if !*should_apply_locally {
                steps.push(Step::Ping(Ping {}));
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
                        target: node.target.clone(),
                    }));
                }

                let (pre_keys, post_keys) = match goal {
                    ApplyGoal::SwitchToConfiguration(SwitchToConfigurationGoal::Switch) => node
                        .keys
                        .clone()
                        .into_iter()
                        .partition(|x| matches!(x.upload_at, UploadKeyAt::PreActivation)),
                    ApplyGoal::Keys => (node.keys.clone(), Vec::new()),
                    _ => unreachable!(),
                };

                if !pre_keys.is_empty() {
                    steps.push(Step::Keys(Keys {
                        keys: pre_keys,
                        target: if *should_apply_locally {
                            None
                        } else {
                            Some(node.target.clone())
                        },
                        privilege_escalation_command: node.privilege_escalation_command.clone(),
                    }));
                }

                if !post_keys.is_empty() {
                    end.push(Step::Keys(Keys {
                        keys: post_keys,
                        target: if *should_apply_locally {
                            None
                        } else {
                            Some(node.target.clone())
                        },
                        privilege_escalation_command: node.privilege_escalation_command.clone(),
                    }));
                }
            }

            if !matches!(goal, ApplyGoal::Keys) {
                steps.push(Step::Evaluate(Evaluate));
            }

            if !matches!(goal, ApplyGoal::Keys)
                && !should_apply_locally
                && (node.build_remotely || matches!(goal, ApplyGoal::Push))
            {
                steps.push(Step::PushEvaluatedOutput(PushEvaluatedOutput {
                    substitute_on_destination: *substitute_on_destination,
                    target: node.target.clone(),
                }));
            }

            if !matches!(goal, ApplyGoal::Keys | ApplyGoal::Push) {
                steps.push(Step::Build(Build {
                    target: if node.build_remotely && !*should_apply_locally {
                        Some(node.target.clone())
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
                    target: node.target.clone(),
                }));
            }

            if let ApplyGoal::SwitchToConfiguration(goal) = goal {
                steps.push(Step::SwitchToConfiguration(SwitchToConfiguration {
                    goal: *goal,
                    reboot: *reboot,
                    target: if *should_apply_locally {
                        Some(node.target.clone())
                    } else {
                        None
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
                    modifiers: *modifiers,
                    should_quit,
                },
                steps,
                greedy_evaluate: !matches!(&goal, ApplyGoal::Keys),
                ignore_failed_ping: matches!(handle_unreachable, HandleUnreachable::Ignore),
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use crate::{
        SubCommandModifiers, function_name, get_test_path,
        hive::{
            node::{ApplyGoal, HandleUnreachable, Name, Node, Step},
            plan::{Goal, plan_for_node},
            steps::{
                build::Build,
                evaluate::Evaluate,
                keys::{Key, Keys, PushKeyAgent, UploadKeyAt},
                ping::Ping, push::PushBuildOutput,
            },
        },
        location,
    };
    use std::{assert_matches::assert_matches, path::PathBuf};
    use std::{
        env,
        sync::{Arc, atomic::AtomicBool},
    };

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
            location.clone().into(),
            &SubCommandModifiers::default(),
            should_quit.clone(),
        );

        assert_eq!(
            plan.steps,
            vec![
                Evaluate.into(),
                Build { target: None }.into() // TODO: this was previously used in an old test, may lose
                                              // coverage by deleting it.
                                              // Ping { }.into(),
                                              // PushKeyAgent { host_platform: "x86_64-linux".into(), substitute_on_destination: true, target: Target::default() }.into(),
                                              // Keys { .. }.into(),
                                              // crate::hive::steps::evaluate::Evaluate.into(),
                                              // crate::hive::steps::build::Build { .. }.into(),
                                              // crate::hive::steps::push::PushBuildOutput { .. }.into(),
                                              // SwitchToConfiguration { .. }.into(),
                                              // Keys {
                                              //     filter: UploadKeyAt::PostActivation
                                              // }
                                              // .into(),
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
        let plan = plan_for_node(
            &node,
            name.clone(),
            &Goal::Apply {
                goal: ApplyGoal::Build,
                should_apply_locally: false,
                no_keys: true,
                substitute_on_destination: true,
                reboot: false,
                host_platform: "x86_64-linux".into(),
                handle_unreachable: HandleUnreachable::default(),
            },
            location.clone().into(),
            &SubCommandModifiers::default(),
            should_quit.clone(),
        );

        assert_eq!(
            plan.steps,
            vec![
                Ping {}.into(),
                crate::hive::steps::evaluate::Evaluate.into(),
                crate::hive::steps::push::PushEvaluatedOutput {
                    substitute_on_destination: true,
                    target: node.target.clone()
                }
                .into(),
                crate::hive::steps::build::Build {
                    target: Some(node.target.clone())
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
            &Goal::Apply {
                goal: ApplyGoal::Build,
                should_apply_locally: false,
                no_keys: true,
                substitute_on_destination: true,
                reboot: false,
                host_platform: "x86_64-linux".into(),
                handle_unreachable: HandleUnreachable::default(),
            },
            location.clone().into(),
            &SubCommandModifiers::default(),
            should_quit.clone(),
        );

        assert_eq!(
            plan.steps,
            vec![
                Ping {}.into(),
                crate::hive::steps::evaluate::Evaluate.into(),
                crate::hive::steps::build::Build { target: None }.into(),
                crate::hive::steps::push::PushBuildOutput {
                    substitute_on_destination: true,
                    target: node.target.clone()
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
                Key {
                    upload_at: UploadKeyAt::PreActivation,
                    ..Default::default()
                }
                .into(),
                Key {
                    upload_at: UploadKeyAt::PostActivation,
                    ..Default::default()
                }
                .into(),
            ],
            ..Default::default()
        };
        let name = &Name(function_name!().into());
        let should_quit = Arc::new(AtomicBool::new(false));
        let plan_apply_keys = plan_for_node(
            &node.clone(),
            name.clone(),
            &Goal::Apply {
                goal: ApplyGoal::Keys,
                should_apply_locally: false,
                no_keys: false,
                substitute_on_destination: true,
                reboot: false,
                host_platform: "x86_64-linux".into(),
                handle_unreachable: HandleUnreachable::default(),
            },
            location.clone().into(),
            &SubCommandModifiers::default(),
            should_quit.clone(),
        );

        assert_eq!(
            plan_apply_keys.steps,
            vec![
                Ping {}.into(),
                PushKeyAgent {
                    substitute_on_destination: true,
                    target: node.target.clone(),
                    host_platform: node.host_platform.clone()
                }
                .into(),
                Keys {
                    target: Some(node.target.clone()),
                    keys: node.keys.clone(),
                    privilege_escalation_command: node.privilege_escalation_command.clone()
                }
                .into(),
            ]
        );

        // Test that keys are split by their `upload_at`, also tests that key
        // step's `target` abides by should_apply_locally
        let plan_activate_with_keys = plan_for_node(
            &node,
            name.clone(),
            &Goal::Apply {
                goal: ApplyGoal::SwitchToConfiguration(
                    crate::hive::node::SwitchToConfigurationGoal::Switch,
                ),
                should_apply_locally: true,
                no_keys: false,
                substitute_on_destination: true,
                reboot: false,
                host_platform: "x86_64-linux".into(),
                handle_unreachable: HandleUnreachable::default(),
            },
            location.clone().into(),
            &SubCommandModifiers::default(),
            should_quit.clone(),
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
    //
    // #[tokio::test]
    // async fn order_build_only() {
    //     let location = location!(get_test_path!());
    //     let mut node = Node::default();
    //
    //     let name = &Name(function_name!().into());
    //     let mut context = Context::create_test_context(location, name, &mut node);
    //
    //     context.objective = Objective::BuildLocally;
    //
    //     let executor = GoalExecutor::new(context);
    //     let steps = get_steps(executor);
    //
    //     assert_eq!(
    //         steps,
    //         vec![
    //             crate::hive::steps::evaluate::Evaluate.into(),
    //             crate::hive::steps::build::Build.into()
    //         ]
    //     );
    // }
    //
    // #[test]
    // fn target_fails_increments() {
    //     let mut target = Target::from_host("localhost");
    //
    //     assert_eq!(target.current_host, 0);
    //
    //     for i in 0..100 {
    //         target.host_failed();
    //         assert_eq!(target.current_host, i + 1);
    //     }
    // }
    //
    // #[test]
    // fn get_preferred_host_fails() {
    //     let mut target = Target {
    //         hosts: vec![
    //             "un.reachable.1".into(),
    //             "un.reachable.2".into(),
    //             "un.reachable.3".into(),
    //             "un.reachable.4".into(),
    //             "un.reachable.5".into(),
    //         ],
    //         ..Default::default()
    //     };
    //
    //     assert_ne!(
    //         target.get_preferred_host().unwrap().to_string(),
    //         "un.reachable.5"
    //     );
    //
    //     for i in 1..=5 {
    //         assert_eq!(
    //             target.get_preferred_host().unwrap().to_string(),
    //             format!("un.reachable.{i}")
    //         );
    //         target.host_failed();
    //     }
    //
    //     for _ in 0..5 {
    //         assert_matches!(
    //             target.get_preferred_host(),
    //             Err(HiveLibError::NetworkError(NetworkError::HostsExhausted))
    //         );
    //     }
    // }
    //
    // #[test]
    // fn test_ssh_opts() {
    //     let target = Target::from_host("hello-world");
    //     let subcommand_modifiers = SubCommandModifiers {
    //         non_interactive: false,
    //         ..Default::default()
    //     };
    //     let tmp = format!(
    //         "/tmp/{}",
    //         rand::distr::SampleString::sample_string(&Alphabetic, &mut rand::rng(), 10)
    //     );
    //
    //     std::fs::create_dir(&tmp).unwrap();
    //
    //     unsafe { env::set_var("XDG_RUNTIME_DIR", &tmp) }
    //
    //     let args = [
    //         "-l".to_string(),
    //         target.user.to_string(),
    //         "-p".to_string(),
    //         target.port.to_string(),
    //         "-o".to_string(),
    //         "StrictHostKeyChecking=accept-new".to_string(),
    //         "-o".to_string(),
    //         "PasswordAuthentication=no".to_string(),
    //         "-o".to_string(),
    //         "KbdInteractiveAuthentication=no".to_string(),
    //     ];
    //
    //     assert_eq!(
    //         target.create_ssh_args(subcommand_modifiers, false).unwrap(),
    //         args
    //     );
    //     assert_eq!(
    //         target.create_ssh_opts(subcommand_modifiers).unwrap(),
    //         args.join(" ")
    //     );
    //
    //     assert_eq!(
    //         target.create_ssh_args(subcommand_modifiers, false).unwrap(),
    //         [
    //             "-l".to_string(),
    //             target.user.to_string(),
    //             "-p".to_string(),
    //             target.port.to_string(),
    //             "-o".to_string(),
    //             "StrictHostKeyChecking=accept-new".to_string(),
    //             "-o".to_string(),
    //             "PasswordAuthentication=no".to_string(),
    //             "-o".to_string(),
    //             "KbdInteractiveAuthentication=no".to_string(),
    //         ]
    //     );
    //
    //     assert_eq!(
    //         target.create_ssh_args(subcommand_modifiers, true).unwrap(),
    //         [
    //             "-l".to_string(),
    //             target.user.to_string(),
    //             "-p".to_string(),
    //             target.port.to_string(),
    //             "-o".to_string(),
    //             "StrictHostKeyChecking=accept-new".to_string(),
    //             "-o".to_string(),
    //             "PasswordAuthentication=no".to_string(),
    //             "-o".to_string(),
    //             "KbdInteractiveAuthentication=no".to_string(),
    //         ]
    //     );
    //
    //     // forced non interactive is the same as --non-interactive
    //     assert_eq!(
    //         target.create_ssh_args(subcommand_modifiers, true).unwrap(),
    //         target
    //             .create_ssh_args(
    //                 SubCommandModifiers {
    //                     non_interactive: true,
    //                     ..Default::default()
    //                 },
    //                 false
    //             )
    //             .unwrap()
    //     );
    // }
    //
    // #[tokio::test]
    // async fn context_quits_sigint() {
    //     let location = location!(get_test_path!());
    //     let mut node = Node::default();
    //
    //     let name = &Name(function_name!().into());
    //     let context = Context::create_test_context(location, name, &mut node);
    //     context
    //         .should_quit
    //         .store(true, std::sync::atomic::Ordering::Relaxed);
    //     let executor = GoalExecutor::new(context);
    //     let status = executor.execute().await;
    //
    //     assert_matches!(status, Err(HiveLibError::Sigint));
    // }
}
