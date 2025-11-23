// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

use std::fmt::Display;


use crate::{
    errors::HiveLibError,
    hive::node::{Context, ExecuteStep},
};

#[derive(PartialEq, Debug)]
pub(crate) struct CleanUp;

impl Display for CleanUp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Clean up")
    }
}

impl ExecuteStep for CleanUp {
    fn should_execute(&self, ctx: &Context) -> bool {
        !ctx.should_apply_locally
    }

    async fn execute(&self, _ctx: &mut Context<'_>) -> Result<(), HiveLibError> {
        Ok(())
    }
}
