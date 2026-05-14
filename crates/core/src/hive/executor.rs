use crate::{
    hive::node::Step,
    status::{NodeStatus, UI_SENDER, UiMessage},
};
use std::{assert_matches::debug_assert_matches, sync::Arc};

use tracing::{Instrument, Span, debug, error, event, instrument};

use crate::{
    EvalGoal, SubCommandModifiers,
    commands::common::evaluate_hive_attribute,
    errors::HiveLibError,
    hive::{
        HiveLocation,
        node::{Context, Derivation, ExecuteStep, Name},
        plan::NodePlan,
    },
};

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
    tx: tokio::sync::oneshot::Sender<Result<Derivation, HiveLibError>>,
    hive_location: Arc<HiveLocation>,
    name: Name,
    modifiers: SubCommandModifiers,
) {
    let output = evaluate_hive_attribute(
        &hive_location,
        &EvalGoal::GetTopLevel(&name),
        Some(&name),
        modifiers,
    )
    .await
    .and_then(|output| {
        serde_json::from_str(&output).map_err(|e| {
            HiveLibError::HiveInitialisationError(
                crate::errors::HiveInitialisationError::ParseEvaluateError(e),
            )
        })
    });

    debug!(output = ?output, done = true);

    let _ = tx.send(output);
}

/// Iterates and executes the steps in the plan.
/// Performs some optimisations such as greedily executing evaluation before
/// other steps independent of evaluation's result.
#[instrument(skip_all, fields(node = %plan.context.name))]
pub async fn execute(mut plan: NodePlan) -> Result<(), HiveLibError> {
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
                plan.context.modifiers,
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
                NodeStatus::Running(step.to_string()),
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
    use std::{assert_matches::assert_matches, path::PathBuf};
    use std::{
        env,
        sync::{Arc, atomic::AtomicBool},
    };

    #[tokio::test]
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
            &SubCommandModifiers::default(),
            should_quit.clone(),
        );

        let status = execute(plan).await;

        assert_matches!(status, Err(HiveLibError::Sigint));
    }
}
