// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

use std::fmt::Display;

use tracing::instrument;

use crate::{
    HiveLibError,
    hive::node::{Context, ExecuteStep},
};

#[derive(Debug, PartialEq)]
pub struct Evaluate;

impl Display for Evaluate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Evaluate")
    }
}

impl ExecuteStep for Evaluate {
    #[instrument(skip_all, name = "eval")]
    async fn execute(&self, ctx: &mut Context) -> Result<(), HiveLibError> {
        let rx = ctx.state.evaluation_rx.take().unwrap();

        ctx.state.evaluation = Some(rx.await.unwrap()?);

        Ok(())
    }
}
