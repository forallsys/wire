// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

use std::fmt::Display;

use tracing::{debug, info, instrument};
use wire_nix_client::{DerivedPath, DerivedPathOutput, NixClient, NixDaemonClientError};

use crate::{
    HiveLibError, SafeStorePath, acquire_stdin_lock,
    commands::{
        CommandArguments, Either, WireCommandChip, builder::CommandStringBuilder,
        run_command_with_env, trace_nix_log_message,
    },
    hive::{
        FlakePrefetch, HiveLocation,
        executor::{BuildOutputHandle, EvaluationOutputHandle},
        node::{Context, ExecuteStep, SharedTarget},
    },
    open_remote_client,
};

const SYSTEM_OUTPUT: &str = "out";

#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq))]
pub(crate) enum NixCommandBuildMetadata {
    Locally {
        cached_derivation: Option<EvaluationOutputHandle>,
    },
    Remotely {
        target: SharedTarget,
        derivation: EvaluationOutputHandle,
    },
}

#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq))]
pub(crate) enum BuildMetadata {
    NixCommand(NixCommandBuildMetadata),
    BuildWithNixDaemon {
        target: Option<SharedTarget>,
        derivation: EvaluationOutputHandle,
    },
}

#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq))]
pub struct Build {
    pub(crate) metadata: BuildMetadata,
    /// the handle this step places its produced path to
    pub(crate) output: BuildOutputHandle,
}

impl Display for Build {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Build")
    }
}

impl ExecuteStep for Build {
    #[allow(clippy::too_many_lines)]
    #[instrument(skip_all, name = "build")]
    async fn execute(&self, ctx: &mut Context) -> Result<(), HiveLibError> {
        match &self.metadata {
            BuildMetadata::BuildWithNixDaemon { target, derivation } => {
                let top_level = derivation.require().await?;

                // use experimental nix daemon client
                let mut connection = if let Some(target) = target {
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
                    Either::Left(ref mut conn) => {
                        conn.query_derivation_output_map(&top_level).await
                    }
                    Either::Right(ref mut conn) => {
                        conn.query_derivation_output_map(&top_level).await
                    }
                }
                .map_err(|err| HiveLibError::NixBuildError {
                    name: ctx.name.clone(),
                    source: err,
                })?;

                debug!(output_map = ?output_map, "got output map");

                let output_path = output_map.remove(SYSTEM_OUTPUT).flatten().ok_or(
                    HiveLibError::NixBuildError {
                        name: ctx.name.clone(),
                        source: NixDaemonClientError::NixDaemonInvalidResponse(format!(
                            "Derivation {top_level:?} did not have output {SYSTEM_OUTPUT:?}"
                        )),
                    },
                )?;

                let derived_path = DerivedPath {
                    store_path: &top_level,
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
                let clobber_guard = acquire_stdin_lock().await;
                println!("{}", output_path.to_absolute_path());
                drop(clobber_guard);

                self.output.set(output_path).await;
            }
            BuildMetadata::NixCommand(metadata) => {
                let attribute = match metadata {
                    NixCommandBuildMetadata::Remotely { derivation, .. }
                    | NixCommandBuildMetadata::Locally {
                        cached_derivation: Some(derivation),
                    } => {
                        format!(
                            "{}^{SYSTEM_OUTPUT}",
                            derivation.require().await?.to_absolute_path()
                        )
                    }
                    NixCommandBuildMetadata::Locally {
                        cached_derivation: None,
                    } => match &*ctx.hive_location {
                        HiveLocation::Flake {
                            prefetch: FlakePrefetch { store_path, .. },
                            ..
                        } => {
                            format!(
                                "{}#wire.nodes.{}.config.system.build.toplevel",
                                store_path.to_absolute_path(),
                                ctx.name
                            )
                        }
                        HiveLocation::HiveNix(path) => {
                            format!(
                                "--file {} nodes.{}.config.system.build.toplevel",
                                path.to_string_lossy(),
                                ctx.name
                            )
                        }
                    },
                };

                // use regular nix build command
                let mut command_string = CommandStringBuilder::nix();
                command_string.args(&[
                    "--extra-experimental-features",
                    "nix-command",
                    "build",
                    "--no-link",
                    "--print-out-paths",
                ]);
                command_string.opt_arg(ctx.modifiers.print_build_logs, "--print-build-logs");
                command_string.arg(&attribute);

                let status = run_command_with_env(
                    &CommandArguments::new(command_string, ctx.modifiers)
                        // build remotely if asked for AND we isnt applying locally
                        .execute_on_remote(match metadata {
                            NixCommandBuildMetadata::Remotely { target, .. } => {
                                Some(target.clone())
                            }
                            NixCommandBuildMetadata::Locally { .. } => None,
                        })
                        .mode(crate::commands::ChildOutputMode::Nix)
                        .log_stdout(),
                    std::collections::HashMap::new(),
                )
                .await?
                .wait_till_success()
                .await
                .map_err(|source| HiveLibError::NixBuildCliError {
                    name: ctx.name.clone(),
                    source,
                })?;

                let stdout = match status {
                    Either::Left((_, stdout)) | Either::Right((_, stdout)) => stdout,
                };

                info!("Built output: {stdout:?}");

                let clobber_guard = acquire_stdin_lock().await;
                println!("{stdout}");
                drop(clobber_guard);

                self.output
                    .set(SafeStorePath::<String>::from_absolute_path(
                        stdout.as_bytes(),
                    )?)
                    .await;
            }
        }

        Ok(())
    }
}
