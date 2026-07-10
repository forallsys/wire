// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

use std::{fmt::Display, sync::Arc};

use tracing::instrument;

use crate::{
    HiveLibError,
    hive::{
        node::{Context, ExecuteStep, Push, SharedTarget},
        plan::{
            AnyNodeOutput, AnyNodeOutputSliceExt, BuildNodeOutput, EvaluationNodeOutput,
            PushBuildOutput, PushDerivationOutput,
        },
    },
};

#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq, Eq))]
pub enum PushOutputKind {
    Evaluation,
    Build,
}

#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq))]
pub struct PushOutput {
    pub substitute_on_destination: bool,
    pub kind: PushOutputKind,
}

impl Display for PushOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Push {} output",
            match self.kind {
                PushOutputKind::Evaluation => "evaluation",
                PushOutputKind::Build => "build",
            }
        )
    }
}

impl ExecuteStep for PushOutput {
    #[instrument(skip_all, name = "push")]
    async fn execute_impl(
        self,
        inputs: Vec<AnyNodeOutput>,
        ctx: Arc<Context>,
    ) -> Result<AnyNodeOutput, HiveLibError> {
        let push = match &self.kind {
            PushOutputKind::Evaluation => {
                Push::Derivation(&inputs.require::<EvaluationNodeOutput>()?.0)
            }
            PushOutputKind::Build => Push::Path(&inputs.require::<BuildNodeOutput>()?.0),
        };

        let target = inputs.require::<SharedTarget>()?;

        if ctx.modifiers.experimental_nix_client {
            crate::push_with_daemon(&ctx, &target, push.clone(), self.substitute_on_destination)
                .await?;
        } else {
            crate::commands::common::push(
                &ctx,
                &target,
                push.clone(),
                self.substitute_on_destination,
            )
            .await?;
        }

        match push {
            Push::Derivation(path) => Ok(AnyNodeOutput::PushDerivation(
                PushDerivationOutput(path.clone()).into(),
            )),
            Push::Path(path) => Ok(AnyNodeOutput::PushBuildOutput(
                PushBuildOutput(path.clone()).into(),
            )),
        }
    }
}
