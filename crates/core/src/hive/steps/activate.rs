// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

use std::fmt::Display;

use tracing::{error, info, instrument, warn};

use crate::{
    HiveLibError,
    commands::{CommandArguments, WireCommandChip, run_command},
    errors::{ActivationError, NetworkError},
    hive::node::{Context, ExecuteStep, Goal, SwitchToConfigurationGoal},
};

#[derive(Debug, PartialEq)]
pub struct SwitchToConfiguration;

impl Display for SwitchToConfiguration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "switch-to-configuration")
    }
}

async fn wait_for_ping(ctx: &Context<'_>) -> Result<(), HiveLibError> {
    let host = ctx.node.target.get_preferred_host()?;
    let mut result = ctx.node.ping(ctx.modifiers).await;

    for num in 0..2 {
        warn!("Trying to ping {host} (attempt {}/3)", num + 1);

        result = ctx.node.ping(ctx.modifiers).await;

        if result.is_ok() {
            info!("Regained connection to {} via {host}", ctx.name);

            break;
        }
    }

    result
}

async fn set_profile(
    goal: SwitchToConfigurationGoal,
    built_path: &String,
    ctx: &Context<'_>,
) -> Result<(), HiveLibError> {
    info!("Setting profiles in anticipation for switch-to-configuration {goal}");

    let command_string = format!("nix-env -p /nix/var/nix/profiles/system/ --set {built_path}");

    let child = run_command(
        &CommandArguments::new(command_string, ctx.modifiers)
            .mode(crate::commands::ChildOutputMode::Nix)
            .on_target(if ctx.should_apply_locally {
                None
            } else {
                Some(&ctx.node.target)
            })
            .elevated(ctx.node),
    )
    .await?;

    let _ = child
        .wait_till_success()
        .await
        .map_err(HiveLibError::CommandError)?;

    info!("Set system profile");

    Ok(())
}

impl ExecuteStep for SwitchToConfiguration {
    fn should_execute(&self, ctx: &Context) -> bool {
        matches!(ctx.goal, Goal::SwitchToConfiguration(..))
    }

    #[instrument(skip_all, name = "activate")]
    async fn execute(&self, ctx: &mut Context<'_>) -> Result<(), HiveLibError> {
        let built_path = ctx.state.build.as_ref().unwrap();

        let Goal::SwitchToConfiguration(goal) = &ctx.goal else {
            unreachable!("Cannot reach as guarded by should_execute")
        };

        if matches!(
            goal,
            // switch profile if switch or boot
            // https://github.com/NixOS/nixpkgs/blob/a2c92aa34735a04010671e3378e2aa2d109b2a72/pkgs/by-name/ni/nixos-rebuild-ng/src/nixos_rebuild/services.py#L224
            SwitchToConfigurationGoal::Switch | SwitchToConfigurationGoal::Boot
        ) {
            set_profile(*goal, built_path, ctx).await?;
        }

        info!("Running switch-to-configuration {goal}");

        let command_string = format!(
            "{built_path}/bin/switch-to-configuration {}",
            match goal {
                SwitchToConfigurationGoal::Switch => "switch",
                SwitchToConfigurationGoal::Boot => "boot",
                SwitchToConfigurationGoal::Test => "test",
                SwitchToConfigurationGoal::DryActivate => "dry-activate",
            }
        );

        let child = run_command(
            &CommandArguments::new(command_string, ctx.modifiers)
                .on_target(if ctx.should_apply_locally {
                    None
                } else {
                    Some(&ctx.node.target)
                })
                .elevated(ctx.node)
                .log_stdout(),
        )
        .await?;

        let result = child.wait_till_success().await;

        match result {
            Ok(_) => {
                if !ctx.reboot {
                    return Ok(());
                }

                if ctx.should_apply_locally {
                    error!("Refusing to reboot local machine!");

                    return Ok(());
                }

                warn!("Rebooting {name}!", name = ctx.name);

                let reboot = run_command(
                    &CommandArguments::new("reboot now", ctx.modifiers)
                        .log_stdout()
                        .on_target(Some(&ctx.node.target))
                        .elevated(ctx.node),
                )
                .await?;

                // consume result, impossible to know if the machine failed to reboot or we
                // simply disconnected
                let _ = reboot
                    .wait_till_success()
                    .await
                    .map_err(HiveLibError::CommandError)?;

                info!("Rebooted {name}, waiting to reconnect...", name = ctx.name);

                if wait_for_ping(ctx).await.is_ok() {
                    return Ok(());
                }

                error!(
                    "Failed to get regain connection to {name} via {host} after reboot.",
                    name = ctx.name,
                    host = ctx.node.target.get_preferred_host()?
                );

                return Err(HiveLibError::NetworkError(
                    NetworkError::HostUnreachableAfterReboot(
                        ctx.node.target.get_preferred_host()?.to_string(),
                    ),
                ));
            }
            Err(error) => {
                warn!(
                    "Activation command for {name} exited unsuccessfully.",
                    name = ctx.name
                );

                // Bail if the command couldn't of broken the system
                // and don't try to regain connection to localhost
                if matches!(goal, SwitchToConfigurationGoal::DryActivate)
                    || ctx.should_apply_locally
                {
                    return Err(HiveLibError::ActivationError(
                        ActivationError::SwitchToConfigurationError(*goal, ctx.name.clone(), error),
                    ));
                }

                if wait_for_ping(ctx).await.is_ok() {
                    return Err(HiveLibError::ActivationError(
                        ActivationError::SwitchToConfigurationError(*goal, ctx.name.clone(), error),
                    ));
                }

                error!(
                    "Failed to get regain connection to {name} via {host} after {goal} activation.",
                    name = ctx.name,
                    host = ctx.node.target.get_preferred_host()?
                );

                return Err(HiveLibError::NetworkError(
                    NetworkError::HostUnreachableAfterReboot(
                        ctx.node.target.get_preferred_host()?.to_string(),
                    ),
                ));
            }
        }
    }
}
