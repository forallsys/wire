use std::{collections::HashMap, sync::Arc};

use serde::{Deserialize, Serialize};
use wire_core::hive::{
    node::{Derivation, Name, Target},
    steps::keys::UploadKeyAt,
};
use wire_keys::{Key, KeyStore};

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
    pub target: Target,
    pub build_remotely: bool,
    pub allow_local_deployment: bool,
    pub tags: im::HashSet<String>,
    pub keys: Vec<Arc<Key>>,
    pub host_platform: Arc<str>,
    pub privilege_escalation_command: im::Vector<Arc<str>>,
}

#[derive(Clone)]
enum SwitchToConfigurationGoal {
    Switch,
    Build,
    Boot,
    Test,
    DryActivate,
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
    },
    Build,
}

enum Step {
    Ping,
    PushKeyAgent,
    Keys { keys: Vec<Arc<Key>> },
    Evaluate,
    PushEvaluatedOutput,
    Build { on_target: bool },
    PushBuildOutput,
    SwitchToConfiguration { goal: SwitchToConfigurationGoal },
}

struct Context {
    derivation: Option<Derivation>,
    build_output: Option<String>,
}

struct NodePlan {
    context: Context,
    steps: Vec<Step>,
    node: Node,
    name: Name,
}

fn create_plans<'a>(nodes: HashMap<Name, NodeRepr>, goal: &'_ Goal) -> (Vec<NodePlan>, KeyStore) {
    let mut key_store = KeyStore::default();
    let mut plans = Vec::new();

    for (name, node) in nodes {
        let mut keys = Vec::with_capacity(node.keys.len());

        for key in node.keys {
            let key = key_store.insert(key);
            keys.push(key);
        }

        let plan = plan_for_node(
            Node {
                target: node.target,
                build_remotely: node.build_remotely,
                allow_local_deployment: node.allow_local_deployment,
                tags: node.tags,
                host_platform: node.host_platform,
                privilege_escalation_command: node.privilege_escalation_command,
                keys,
            },
            name,
            goal,
        );

        plans.push(plan);
    }

    (plans, key_store)
}

fn plan_for_node<'a>(node: Node, name: Name, goal: &'_ Goal) -> NodePlan {
    match goal {
        Goal::Build => NodePlan {
            context: Context {
                derivation: None,
                build_output: None,
            },
            steps: vec![Step::Evaluate, Step::Build { on_target: false }],
            node,
            name,
        },
        Goal::Apply {
            goal: apply_goal,
            should_apply_locally,
            no_keys,
        } => {
            let mut steps: Vec<Step> = Vec::new();

            if !*should_apply_locally {
                steps.push(Step::Ping);
            }

            if !*no_keys
                && matches!(
                    &apply_goal,
                    ApplyGoal::Keys
                        | ApplyGoal::SwitchToConfiguration(SwitchToConfigurationGoal::Switch)
                )
            {
                steps.push(Step::PushKeyAgent);

                match apply_goal {
                    ApplyGoal::SwitchToConfiguration(SwitchToConfigurationGoal::Switch) => {
                        steps.push(Step::Keys {
                            keys: node
                                .keys
                                .iter()
                                .filter(|x| matches!(x.upload_at, UploadKeyAt::PreActivation))
                                .cloned()
                                .collect(),
                        });
                    }
                    ApplyGoal::Keys => {
                        steps.push(Step::Keys {
                            keys: node.keys.clone(),
                        });
                    }
                    _ => unreachable!(),
                }
            }

            if matches!(
                goal,
                Goal::Apply {
                    goal: ApplyGoal::Keys,
                    ..
                } | Goal::Build
            ) {
                steps.push(Step::Evaluate);
            }

            let should_push_evaluate = match goal {
                Goal::Apply {
                    goal,
                    should_apply_locally,
                    ..
                } => {
                    !matches!(goal, ApplyGoal::Keys)
                        && !should_apply_locally
                        && (node.build_remotely | matches!(goal, ApplyGoal::Push))
                }
                Goal::Build => false,
            };

            if should_push_evaluate {
                steps.push(Step::PushEvaluatedOutput);
            }

            let should_build = match goal {
                Goal::Apply { goal, .. } => !matches!(goal, ApplyGoal::Keys | ApplyGoal::Push),
                Goal::Build => true,
            };

            if should_build {
                steps.push(Step::Build {
                    on_target: if node.build_remotely
                        && let Goal::Apply {
                            should_apply_locally,
                            ..
                        } = goal
                        && !*should_apply_locally
                    {
                        true
                    } else {
                        false
                    },
                });
            }

            if let Goal::Apply { goal, should_apply_locally, .. } = goal {
                if !node.build_remotely && !should_apply_locally && !matches!(goal, ApplyGoal::Keys | ApplyGoal::Push) {
                    steps.push(Step::PushBuildOutput);
                }

                if let ApplyGoal::SwitchToConfiguration(goal) = goal {
                    steps.push(Step::SwitchToConfiguration { goal: goal.clone() });
                }
            }

            NodePlan {
                context: Context {
                    derivation: None,
                    build_output: None,
                },
                steps,
                node,
                name,
            }
        }
    }
}
