// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

use std::fmt::Display;

use tracing::{Level, event, instrument};

use crate::{
    HiveLibError,
    hive::node::{Context, ExecuteStep, Objective},
};

#[derive(Debug, PartialEq)]
pub struct Ping;

impl Display for Ping {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Ping node")
    }
}

impl ExecuteStep for Ping {
    fn should_execute(&self, ctx: &Context) -> bool {
        todo!()
    }

    #[instrument(skip_all, name = "ping")]
    async fn execute(&self, ctx: &mut Context<'_>) -> Result<(), HiveLibError> {
        todo!()
    }
}
