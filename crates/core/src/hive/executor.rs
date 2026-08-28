use crate::{
    SafeStorePath,
    hive::node::Step,
    status::{NodeStatus, UI_SENDER, UiMessage},
};
use std::debug_assert_matches;
use std::sync::Arc;

use tokio::sync::{RwLock, oneshot};
use tracing::{Instrument, Span, debug, error, event, instrument};

use crate::{
    EvalGoal, SubCommandModifiers,
    commands::common::evaluate_hive_attribute,
    errors::HiveLibError,
    hive::{
        HiveLocation,
        node::{Context, ExecuteStep, Name},
        plan::NodePlan,
    },
};

/// A shared struct that "Store Path Producing" steps place outputs into.
///
/// Steps such as Build, Evaluate, `PushKeyAgent` write their store path into
/// this struct, and "Store Path Consuming" steps can
/// read from (Build reads from Evaluate, Keys reads from `PushKeyAgent`, etc)
///
/// Cloning this value will keep pointing to the same handle.
#[derive(Debug, Clone)]
pub struct OutputHandle {
    inner: Arc<RwLock<Option<SafeStorePath<String>>>>,
}

pub type BuildOutputHandle = OutputHandle;
pub type EvaluationOutputHandle = OutputHandle;
pub type KeyAgentPathHandle = OutputHandle;

#[cfg(test)]
impl PartialEq for OutputHandle {
    fn eq(&self, other: &Self) -> bool {
        if Arc::ptr_eq(&self.inner, &other.inner) {
            return true;
        }

        // fall back to comparing the inner store path
        match (self.inner.try_read(), other.inner.try_read()) {
            (Ok(a), Ok(b)) => *a == *b,
            _ => false,
        }
    }
}

#[cfg(test)]
impl Eq for OutputHandle {}

impl OutputHandle {
    /// Create a handle for which the store path is not already known
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(None)),
        }
    }

    /// Create a handle for a store path which is already known
    pub(crate) fn new_known(store_path: SafeStorePath<String>) -> Self {
        Self {
            inner: Arc::new(RwLock::new(Some(store_path))),
        }
    }

    pub(crate) async fn set(&self, store_path: SafeStorePath<String>) {
        *self.inner.write().await = Some(store_path);
    }

    pub(crate) async fn get(&self) -> Option<SafeStorePath<String>> {
        self.inner.read().await.clone()
    }

    /// Attempt to read the previously written store path. If the store path
    /// producing step was never planned, this will fail.
    pub(crate) async fn require(&self) -> Result<SafeStorePath<String>, HiveLibError> {
        self.get().await.ok_or(HiveLibError::MissingStepOutput)
    }
}

/// returns Err if the application should shut down.
fn app_shutdown_guard(context: &Context) -> Result<(), HiveLibError> {
    if context
        .should_quit
        .load(std::sync::atomic::Ordering::Relaxed)
    {
        return Err(HiveLibError::Sigint);
    }

    Ok(())
}

/// Task that evaluates the node.
#[instrument(skip_all, name = "eval")]
async fn evaluate_task(
    tx: tokio::sync::oneshot::Sender<Result<SafeStorePath<String>, HiveLibError>>,
    hive_location: Arc<HiveLocation>,
    name: Name,
    modifiers: Arc<SubCommandModifiers>,
    on_new_evaluation: oneshot::Sender<SafeStorePath<String>>,
) {
    let output = evaluate_hive_attribute(&hive_location, &EvalGoal::GetTopLevel(&name), modifiers)
        .await
        .and_then(|output| {
            serde_json::from_str(&output).map_err(|e| {
                HiveLibError::HiveInitialisationError(
                    crate::errors::HiveInitialisationError::ParseEvaluateError(e),
                )
            })
        })
        .and_then(|output: String| {
            debug!(pre_parsed_output = %output, "evaluated {name}");

            SafeStorePath::<String>::from_absolute_path(output.as_bytes())
                .map_err(HiveLibError::StorePath)
        });

    debug!(output = ?output, done = true);

    if let Ok(ref path) = output {
        let _ = on_new_evaluation.send(path.clone());
    }

    let _ = tx.send(output);
}

/// Iterates and executes the steps in the plan.
/// Performs some optimisations such as greedily executing evaluation before
/// other steps independent of evaluation's result.
#[instrument(skip_all, fields(node = %plan.context.name))]
pub async fn execute(
    mut plan: NodePlan,
    on_new_evaluation: oneshot::Sender<SafeStorePath<String>>,
) -> Result<(), HiveLibError> {
    app_shutdown_guard(&plan.context)?;

    let (tx, rx) = tokio::sync::oneshot::channel();
    plan.context.state.evaluation_rx = Some(rx);

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

    if plan.greedy_evaluate {
        tokio::spawn(
            evaluate_task(
                tx,
                plan.context.hive_location.clone(),
                plan.context.name.clone(),
                plan.context.modifiers.clone(),
                on_new_evaluation,
            )
            .in_current_span(),
        );
    }

    let length = plan.steps.len();

    for (position, step) in plan.steps.iter().enumerate() {
        app_shutdown_guard(&plan.context)?;

        event!(
            tracing::Level::INFO,
            step = step.to_string(),
            progress = format!("{}/{length}", position + 1)
        );

        if let Some(tx) = UI_SENDER.get() {
            let _ = tx.send(UiMessage::SetStatus(
                plan.context.name.clone(),
                NodeStatus::Running {
                    status: step.to_string(),
                    last_log: None,
                },
            ));
        }

        if let Err(err) = step.execute(&mut plan.context).await.inspect_err(|_| {
            error!("Failed to execute `{step}`");
        }) {
            if matches!(step, Step::Ping(..)) && plan.ignore_failed_ping {
                return Ok(());
            }

            if let Some(tx) = UI_SENDER.get() {
                let _ = tx.send(UiMessage::SetStatus(
                    plan.context.name.clone(),
                    NodeStatus::Failed,
                ));
            }

            return Err(err);
        }
    }

    if let Some(tx) = UI_SENDER.get() {
        let _ = tx.send(UiMessage::SetStatus(
            plan.context.name.clone(),
            NodeStatus::Succeeded,
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use tokio::sync::oneshot;

    use crate::{
        SubCommandModifiers,
        errors::HiveLibError,
        function_name, get_test_path,
        hive::{
            executor::execute,
            node::{ApplyGoal, HandleUnreachable, Name, Node, SwitchToConfigurationGoal},
            plan::{ApplyGoalArgs, Goal, plan_for_node},
        },
        location,
    };
    use std::assert_matches;
    use std::path::PathBuf;
    use std::{
        env,
        sync::{Arc, atomic::AtomicBool},
    };

    #[tokio::test]
    #[cfg_attr(feature = "no_eval_tests", ignore)]
    async fn plan_executor_quits_sigint() {
        let location = location!(get_test_path!());
        let node = Node::default();
        let name = &Name(function_name!().into());
        let should_quit = Arc::new(AtomicBool::new(true));
        let plan = plan_for_node(
            &node.clone(),
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
            location.clone().into(),
            &Arc::new(SubCommandModifiers::default()),
            should_quit.clone(),
            None,
        );

        let channel = oneshot::channel();

        let status = execute(plan, channel.0).await;

        assert_matches!(status, Err(HiveLibError::Sigint));
    }
}
