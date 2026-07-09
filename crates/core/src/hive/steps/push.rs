// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

use std::fmt::Display;

use tracing::instrument;

use crate::{
    HiveLibError,
    hive::{
        executor::{BuildOutputHandle, EvaluationOutputHandle},
        node::{Context, ExecuteStep, Push, SharedTarget},
    },
};

#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq))]
pub enum PushOutputHandle {
    Evaluation(EvaluationOutputHandle),
    Build(BuildOutputHandle),
}

#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq))]
pub struct PushOutput {
    pub substitute_on_destination: bool,
    pub target: SharedTarget,
    pub path: PushOutputHandle,
}

impl Display for PushOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Push {} output",
            match self.path {
                PushOutputHandle::Evaluation(..) => "evaluation",
                PushOutputHandle::Build(..) => "build",
            }
        )
    }
}

impl ExecuteStep for PushOutput {
    #[instrument(skip_all, name = "push")]
    async fn execute(&self, ctx: &mut Context) -> Result<(), HiveLibError> {
        let push = match &self.path {
            PushOutputHandle::Evaluation(handle) => Push::Derivation(&handle.require().await?),
            PushOutputHandle::Build(handle) => Push::Path(&handle.require().await?),
        };

        if ctx.modifiers.experimental_nix_client {
            crate::push_with_daemon(ctx, &self.target, push, self.substitute_on_destination).await
        } else {
            crate::commands::common::push(ctx, &self.target, push, self.substitute_on_destination)
                .await
        }
    }
}
