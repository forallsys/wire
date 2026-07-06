// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

use std::fmt::Display;

use tracing::instrument;

use crate::{
    HiveLibError,
    hive::{
        executor::EvaluationOutputHandle,
        node::{Context, ExecuteStep},
    },
};

#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq, Eq))]
pub struct Evaluate {
    /// output handle to write to once the greedy eval is complete
    pub output: EvaluationOutputHandle,
}

impl Display for Evaluate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Evaluate the node")
    }
}

impl ExecuteStep for Evaluate {
    #[instrument(skip_all, name = "eval")]
    async fn execute(&self, ctx: &mut Context) -> Result<(), HiveLibError> {
        let rx = ctx.state.evaluation_rx.take().unwrap();

        self.output.set(rx.await.unwrap()?).await;

        Ok(())
    }
}
