// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

use std::fmt::Display;

use tracing::{info, instrument};

use crate::{
    HiveLibError,
    commands::{
        CommandArguments, Either, WireCommandChip, builder::CommandStringBuilder,
        run_command_with_env,
    },
    hive::node::{Context, ExecuteStep, Goal, Objective},
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
        todo!()
    }

    #[instrument(skip_all, name = "build")]
    async fn execute(&self, ctx: &mut Context<'_>) -> Result<(), HiveLibError> {
        todo!()
    }
}
