// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

use std::fmt::Display;

use tracing::instrument;

use crate::{
    HiveLibError,
    commands::common::push,
    hive::node::{Context, ExecuteStep, SharedTarget},
};

#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq))]
pub struct PushEvaluatedOutput {
    pub substitute_on_destination: bool,
    pub target: SharedTarget,
}

#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq))]
pub struct PushBuildOutput {
    pub substitute_on_destination: bool,
    pub target: SharedTarget,
}

impl Display for PushEvaluatedOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Push the evaluated output")
    }
}

impl Display for PushBuildOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Push the build output")
    }
}

impl ExecuteStep for PushEvaluatedOutput {
    #[instrument(skip_all, name = "push_eval")]
    async fn execute(&self, ctx: &mut Context) -> Result<(), HiveLibError> {
        let top_level = ctx.state.evaluation.as_ref().unwrap();

        push(
            ctx,
            &self.target,
            crate::hive::node::Push::Derivation(top_level),
            self.substitute_on_destination,
        )
        .await?;

        Ok(())
    }
}

impl ExecuteStep for PushBuildOutput {
    #[instrument(skip_all, name = "push_build")]
    async fn execute(&self, ctx: &mut Context) -> Result<(), HiveLibError> {
        let built_path = ctx.state.build.as_ref().unwrap();

        push(
            ctx,
            &self.target,
            crate::hive::node::Push::Path(built_path),
            self.substitute_on_destination,
        )
        .await?;

        Ok(())
    }
}
