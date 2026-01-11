// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

use std::fmt::Display;

use tracing::{error, info, instrument, warn};

use crate::{
    HiveLibError,
    commands::{CommandArguments, WireCommandChip, builder::CommandStringBuilder, run_command},
    errors::{ActivationError, NetworkError},
    hive::node::{Context, ExecuteStep, Goal, Objective, SwitchToConfigurationGoal},
};

#[derive(Debug, PartialEq)]
pub struct SwitchToConfiguration;

impl Display for SwitchToConfiguration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "switch-to-configuration")
    }
}

impl ExecuteStep for SwitchToConfiguration {
    fn should_execute(&self, ctx: &Context) -> bool {
        todo!()
    }

    #[allow(clippy::too_many_lines)]
    #[instrument(skip_all, name = "activate")]
    async fn execute(&self, ctx: &mut Context<'_>) -> Result<(), HiveLibError> {
        todo!()
    }
}
