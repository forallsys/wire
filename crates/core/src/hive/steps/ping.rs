// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

use std::{fmt::Display, sync::Arc};

use tracing::{Level, event, instrument};

use crate::{
    HiveLibError,
    hive::{
        node::{Context, ExecuteStep, SharedTarget},
        plan::AnyNodeOutput,
    },
};

#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq))]
pub struct Ping {
    pub target: SharedTarget,
}

impl Display for Ping {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Ping")
    }
}

impl ExecuteStep for Ping {
    #[instrument(skip_all, name = "ping")]
    async fn execute_impl(
        self,
        _inputs: Vec<AnyNodeOutput>,
        ctx: Arc<Context>,
    ) -> Result<AnyNodeOutput, HiveLibError> {
        loop {
            let target = self.target.0.read().await;

            event!(
                Level::INFO,
                status = "attempting",
                host = target.get_preferred_host()?.to_string()
            );

            if target
                .ping(ctx.modifiers, ctx.should_quit.clone())
                .await
                .is_ok()
            {
                event!(
                    Level::INFO,
                    status = "success",
                    host = target.get_preferred_host()?.to_string()
                );

                return Ok(AnyNodeOutput::Ping(self.target.clone().into()));
            }

            // ? will take us out if we ran out of hosts
            event!(
                Level::WARN,
                status = "failed to ping",
                host = target.get_preferred_host()?.to_string()
            );

            drop(target);

            self.target.0.write().await.host_failed();
        }
    }
}
