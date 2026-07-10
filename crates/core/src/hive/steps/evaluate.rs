// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

use std::{fmt::Display, sync::Arc};

use tracing::{debug, instrument};
use wire_nix_client::store_path::SafeStorePath;

use crate::{
    EvalGoal, HiveLibError,
    commands::common::evaluate_hive_attribute,
    hive::{
        node::{Context, ExecuteStep},
        plan::{AnyNodeOutput, EvaluationNodeOutput},
    },
};

#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq, Eq))]
pub struct Evaluate {
    pub cached_evaluation: Option<SafeStorePath<String>>,
}

impl Display for Evaluate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Evaluate")
    }
}

impl ExecuteStep for Evaluate {
    #[instrument(skip_all, name = "eval")]
    async fn execute_impl(
        self,
        _inputs: Vec<AnyNodeOutput>,
        ctx: Arc<Context>,
    ) -> Result<AnyNodeOutput, HiveLibError> {
        if let Some(cached_evaluation) = &self.cached_evaluation {
            return Ok(AnyNodeOutput::Derivation(
                EvaluationNodeOutput(cached_evaluation.clone()).into(),
            ));
        }

        let output = evaluate_hive_attribute(
            &ctx.hive_location,
            &EvalGoal::GetTopLevel(&ctx.name),
            ctx.modifiers,
        )
        .await
        .and_then(|output| {
            serde_json::from_str(&output).map_err(|e| {
                HiveLibError::HiveInitialisationError(
                    crate::errors::HiveInitialisationError::ParseEvaluateError(e),
                )
            })
        })
        .and_then(|output: String| {
            debug!(pre_parsed_output = %output, "evaluated {}", ctx.name);

            SafeStorePath::<String>::from_absolute_path(output.as_bytes())
                .map_err(HiveLibError::StorePath)
        })?;

        debug!(output = ?output, done = true);

        Ok(AnyNodeOutput::Derivation(
            EvaluationNodeOutput(output).into(),
        ))
    }
}
