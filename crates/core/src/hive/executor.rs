use crate::hive::node::Step;
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
    status::STATUS,
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
    let output = evaluate_hive_attribute(&hive_location, &EvalGoal::GetTopLevel(&name), modifiers)
        .await
        .map(|output| {
            serde_json::from_str::<Derivation>(&output).expect("failed to parse derivation")
        });

    debug!(output = ?output, done = true);

    let _ = tx.send(output);
}

/// Iterates and executes the steps in the plan.
/// Performs some optimizations such as greedly executing evaluation before
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

        STATUS
            .lock()
            .set_node_step(&plan.context.name, step.to_string());

        if let Err(err) = step.execute(&mut plan.context).await.inspect_err(|_| {
            error!("Failed to execute `{step}`");
        }) {
            if matches!(step, Step::Ping(..)) && plan.ignore_failed_ping {
                return Ok(());
            }

            STATUS.lock().mark_node_failed(&plan.context.name);

            return Err(err);
        }
    }

    STATUS.lock().mark_node_succeeded(&plan.context.name);

    Ok(())
}
