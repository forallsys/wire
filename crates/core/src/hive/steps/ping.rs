// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

use std::fmt::Display;

use tracing::{Level, event, instrument};

use crate::{
    HiveLibError,
    hive::node::{Context, ExecuteStep},
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
        !ctx.should_apply_locally
    }

    #[instrument(skip_all, name = "ping")]
    async fn execute(&self, ctx: &mut Context<'_>) -> Result<(), HiveLibError> {
        loop {
            event!(
                Level::INFO,
                status = "attempting",
                host = ctx.node.target.get_preferred_host()?.to_string()
            );

            if ctx.node.ping(ctx.modifiers).await.is_ok() {
                event!(
                    Level::INFO,
                    status = "success",
                    host = ctx.node.target.get_preferred_host()?.to_string()
                );
                return Ok(());
            }

            // ? will take us out if we ran out of hosts
            event!(
                Level::WARN,
                status = "failed to ping",
                host = ctx.node.target.get_preferred_host()?.to_string()
            );
            ctx.node.target.host_failed();
        }
    }
}
