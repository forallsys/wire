// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

use std::fmt::Display;

use tracing::{info, instrument};

use crate::{
    HiveLibError, SafeStorePath,
    commands::{
        CommandArguments, Either, WireCommandChip, builder::CommandStringBuilder,
        run_command_with_env,
    },
    hive::node::{Context, ExecuteStep, SharedTarget},
};

#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq))]
pub struct Build {
    pub(crate) target: Option<SharedTarget>,
}

impl Display for Build {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Build the node")
    }
}

impl ExecuteStep for Build {
    #[instrument(skip_all, name = "build")]
    async fn execute(&self, ctx: &mut Context) -> Result<(), HiveLibError> {
        let top_level = ctx.state.evaluation.as_ref().unwrap();

        let mut command_string = CommandStringBuilder::nix();
        command_string.args(&[
            "--extra-experimental-features",
            "nix-command",
            "build",
            "--no-link",
            "--print-out-paths",
        ]);
        command_string.opt_arg(ctx.modifiers.print_build_logs, "--print-build-logs");
        command_string.arg(format!("{}^*", top_level.to_absolute_path()));

        let status = run_command_with_env(
            &CommandArguments::new(command_string, ctx.modifiers)
                // build remotely if asked for AND we arent applying locally
                .execute_on_remote(self.target.clone())
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

        // print built path to stdout
        println!("{stdout}");

        ctx.state.build = Some(SafeStorePath::<String>::from_absolute_path(
            stdout.as_bytes(),
        )?);

        Ok(())
    }
}
