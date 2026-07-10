use crate::{
    hive::plan::PlanGraph,
    status::{NodeStatus, UI_SENDER, UiMessage},
};
use std::debug_assert_matches;
use std::sync::Arc;

use tracing::{Span, instrument};

use crate::{errors::HiveLibError, hive::node::Context};

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

/// Iterates and executes the steps in the plan.
/// Performs some optimisations such as greedily executing evaluation before
/// other steps independent of evaluation's result.
#[instrument(skip_all, fields(node = %plan.context.name))]
pub async fn execute(plan: PlanGraph) -> Result<(), Arc<HiveLibError>> {
    app_shutdown_guard(&plan.context)?;

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

    let name = plan.context.name.clone();
    let sink = plan.get_sink_step()?;
    let result = sink.get_output().await;

    match result {
        Ok(_) => {
            if let Some(tx) = UI_SENDER.get() {
                let _ = tx.send(UiMessage::SetStatus(name, NodeStatus::Succeeded));
            }
            Ok(())
        }
        Err(e) => Err(e),
    }
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
            plan::{ApplyGoalArgs, Goal},
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
            &SubCommandModifiers::default(),
            should_quit.clone(),
            None,
        );

        let channel = oneshot::channel();

        let status = execute(plan, channel.0).await;

        assert_matches!(status, Err(HiveLibError::Sigint));
    }
}
