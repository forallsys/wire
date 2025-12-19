// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

use std::fmt::Display;

use tracing::{info, instrument};

use crate::{
    HiveLibError,
    commands::{CommandArguments, Either, WireCommandChip, run_command_with_env},
    hive::node::{Context, ExecuteStep, Goal},
};

#[derive(Debug, PartialEq)]
pub struct Build;

impl Display for Build {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Build the node")
    }
}

impl ExecuteStep for Build {
    fn should_execute(&self, ctx: &Context) -> bool {
        !matches!(ctx.goal, Goal::Keys | Goal::Push)
    }

    #[instrument(skip_all, name = "build")]
    async fn execute(&self, ctx: &mut Context<'_>) -> Result<(), HiveLibError> {
        let top_level = ctx.state.evaluation.as_ref().unwrap();

        let command_string = format!(
            "nix --extra-experimental-features nix-command \
            build --print-build-logs --no-link --print-out-paths {top_level}"
        );

        let status = run_command_with_env(
            &CommandArguments::new(command_string, ctx.modifiers)
                // build remotely if asked for AND we arent applying locally
                // building remotely but applying locally does not logically
                // make any sense
                .on_target(if ctx.node.build_remotely && !ctx.should_apply_locally {
                    Some(&ctx.node.target)
                } else {
                    None
                })
                .mode(crate::commands::ChildOutputMode::Nix)
                .log_stdout(),
            std::collections::HashMap::new(),
        )
        .await?
        .wait_till_success()
        .await
        .map_err(|source| HiveLibError::NixBuildError {
            name: ctx.name.clone(),
            source,
        })?;

        let stdout = match status {
            Either::Left((_, stdout)) | Either::Right((_, stdout)) => stdout,
        };

        info!("Built output: {stdout:?}");
        ctx.state.build = Some(stdout);

        Ok(())
    }
}
