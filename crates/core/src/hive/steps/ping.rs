// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

use std::fmt::Display;

use tracing::instrument;

use crate::{
    HiveLibError,
    hive::node::{Context, ExecuteStep},
};

#[derive(Debug, PartialEq)]
pub struct Ping {
    // target: Target
}

impl Display for Ping {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Ping node")
    }
}

impl ExecuteStep for Ping {
    #[instrument(skip_all, name = "ping")]
    async fn execute(&self, ctx: &mut Context) -> Result<(), HiveLibError> {
        loop {
            todo!()
            // event!(
            //     Level::INFO,
            //     status = "attempting",
            //     host = self.target.get_preferred_host()?.to_string()
            // );

            // if ctx.node.ping(ctx.modifiers).await.is_ok() {
            //     event!(
            //         Level::INFO,
            //         status = "success",
            //         host = self.target.get_preferred_host()?.to_string()
            //     );
            //     return Ok(());
            // }

            // ? will take us out if we ran out of hosts
            // event!(
            //     Level::WARN,
            //     status = "failed to ping",
            //     host = self.target.get_preferred_host()?.to_string()
            // );
            // self.target.host_failed();
        }
    }
}
