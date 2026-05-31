// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

use std::fmt::Display;

use tracing::{info, instrument};

use crate::{
    HiveLibError, SafeStorePath,
    hive::node::{Context, ExecuteStep},
};

#[derive(Debug, PartialEq)]
pub struct Evaluate {
    /// evaluation that was previously built & cached
    pub cached_evaluation: Option<SafeStorePath<String>>,
}

impl Display for Evaluate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Evaluate the node")
    }
}

impl ExecuteStep for Evaluate {
    #[instrument(skip_all, name = "eval")]
    async fn execute(&self, ctx: &mut Context) -> Result<(), HiveLibError> {
        if let Some(ref cached_evaluation) = self.cached_evaluation {
            info!(
                "Skipping evaluation, cached as {}",
                cached_evaluation.to_absolute_path()
            );
            ctx.state.evaluation = Some(cached_evaluation.clone());
        } else {
            let rx = ctx.state.evaluation_rx.take().unwrap();

            ctx.state.evaluation = Some(rx.await.unwrap()?);
        }

        Ok(())
    }
}
