// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

use std::fmt::Display;

use tracing::{debug, info, instrument};
use wire_nix_client::{DerivedPath, DerivedPathOutput, NixClient, NixDaemonClientError};

use crate::{
    HiveLibError,
    commands::{Either, trace_nix_log_message},
    hive::node::{Context, ExecuteStep, SharedTarget},
    open_remote_client,
};

const SYSTEM_OUTPUT: &str = "out";

#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq))]
pub struct Build {
    pub(crate) target: Option<SharedTarget>,
}

impl Display for Build {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Build the node")
    }
}

impl ExecuteStep for Build {
    #[instrument(skip_all, name = "build")]
    async fn execute(&self, ctx: &mut Context) -> Result<(), HiveLibError> {
        let top_level = ctx.state.evaluation.as_ref().unwrap();

        let mut connection = if let Some(ref target) = self.target {
            let target = target.0.read().await;

            Either::Left(
                open_remote_client(
                    &target,
                    ctx.modifiers,
                    trace_nix_log_message,
                    ctx.should_quit.clone(),
                )
                .await?
                .0,
            )
        } else {
            Either::Right(
                NixClient::open_local(
                    trace_nix_log_message,
                    ctx.should_quit.clone(),
                    ctx.modifiers.print_build_logs,
                )
                .await
                .map_err(HiveLibError::NixDaemonClientError)?,
            )
        };

        let mut output_map = match connection {
            Either::Left(ref mut conn) => conn.query_derivation_output_map(top_level).await,
            Either::Right(ref mut conn) => conn.query_derivation_output_map(top_level).await,
        }
        .map_err(|err| HiveLibError::NixBuildError {
            name: ctx.name.clone(),
            source: err,
        })?;

        debug!(output_map = ?output_map, "got output map");

        let output_path =
            output_map
                .remove(SYSTEM_OUTPUT)
                .flatten()
                .ok_or(HiveLibError::NixBuildError {
                    name: ctx.name.clone(),
                    source: NixDaemonClientError::NixDaemonInvalidResponse(format!(
                        "Derivation {top_level:?} did not have output {SYSTEM_OUTPUT:?}"
                    )),
                })?;

        let derived_path = DerivedPath {
            store_path: top_level,
            outputs: DerivedPathOutput::OutputNames(&[SYSTEM_OUTPUT]),
        };

        match connection {
            Either::Left(mut conn) => conn.build(&vec![derived_path]).await,
            Either::Right(mut conn) => conn.build(&vec![derived_path]).await,
        }
        .map_err(|source| HiveLibError::NixBuildError {
            name: ctx.name.clone(),
            source,
        })?;

        info!("Built output: {output_path:?}");

        // print built path to stdout
        println!("{}", output_path.to_absolute_path());

        ctx.state.build = Some(output_path);

        Ok(())
    }
}
