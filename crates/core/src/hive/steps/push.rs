// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

use std::fmt::Display;

use tracing::instrument;

use crate::{
    HiveLibError,
    commands::common::push,
    hive::node::{Context, ExecuteStep, Goal, Objective},
};

#[derive(Debug, PartialEq)]
pub struct PushEvaluatedOutput;
#[derive(Debug, PartialEq)]
pub struct PushBuildOutput;

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
    fn should_execute(&self, ctx: &Context) -> bool {
        todo!()
    }

    #[instrument(skip_all, name = "push_eval")]
    async fn execute(&self, ctx: &mut Context<'_>) -> Result<(), HiveLibError> {
        todo!()
    }
}

impl ExecuteStep for PushBuildOutput {
    fn should_execute(&self, ctx: &Context) -> bool {
        todo!()
    }

    #[instrument(skip_all, name = "push_build")]
    async fn execute(&self, ctx: &mut Context<'_>) -> Result<(), HiveLibError> {
        todo!()
    }
}
