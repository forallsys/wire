// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

use std::{fmt::Display, sync::Arc, time::Duration};

use tracing::{error, info, instrument, warn};

use crate::{
    HiveLibError, SafeStorePath,
    commands::{CommandArguments, WireCommandChip, builder::CommandStringBuilder, run_command},
    errors::{ActivationError, NetworkError},
    hive::{
        executor::BuildOutputHandle,
        node::{Context, ExecuteStep, SharedTarget, SwitchToConfigurationGoal},
    },
};

#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq))]
pub struct SwitchToConfiguration {
    pub goal: SwitchToConfigurationGoal,
    pub reboot: bool,
    pub target: Option<SharedTarget>,
    pub privilege_escalation_command: Arc<Vec<Arc<str>>>,

    pub top_level: BuildOutputHandle,
}

impl Display for SwitchToConfiguration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Activation")
    }
}

#[allow(clippy::significant_drop_tightening)]
async fn wait_for_ping(target: &SharedTarget, ctx: &Context) -> Result<(), HiveLibError> {
    // try to regain connection for 5-ish minutes after a reboot, waiting 30s between
    // attempts to give the machine time to come back up
    const PING_INTERVAL: Duration = Duration::from_secs(30);
    const MAX_ATTEMPTS: u32 = 10;

    let target = target.0.read().await;
    let host = target.get_preferred_host()?;

    for num in 0..MAX_ATTEMPTS {
        warn!("Trying to ping {host} (attempt {}/{MAX_ATTEMPTS})", num + 1);

        let result = target
            .ping(ctx.modifiers.clone(), ctx.should_quit.clone(), &ctx.name)
            .await;

        if result.is_ok() {
            info!("Regained connection to {} via {host}", ctx.name);

            return Ok(());
        }

        if num + 1 < MAX_ATTEMPTS {
            tokio::time::sleep(PING_INTERVAL).await;
        }
    }

    Err(HiveLibError::NetworkError(NetworkError::HostsExhausted))
}

impl SwitchToConfiguration {
    async fn set_profile(
        &self,
        built_path: &SafeStorePath<String>,
        ctx: &Context,
    ) -> Result<(), HiveLibError> {
        info!(
            "Setting profiles in anticipation for switch-to-configuration {}",
            self.goal
        );

        let mut command_string = CommandStringBuilder::new("nix-env");
        command_string.args(&["-p", "/nix/var/nix/profiles/system", "--set"]);
        command_string.arg(built_path.to_absolute_path());

        let child = run_command(
            &CommandArguments::new(
                command_string,
                ctx.modifiers.clone(),
                Some(ctx.name.clone()),
            )
            .mode(crate::commands::ChildOutputMode::Nix(Some(
                ctx.name.clone(),
            )))
            .execute_on_remote(self.target.clone())
            .privileged(&self.privilege_escalation_command),
        )
        .await?;

        let _ = child
            .wait_till_success()
            .await
            .map_err(HiveLibError::CommandError)?;

        info!("Set system profile");

        Ok(())
    }
}

impl ExecuteStep for SwitchToConfiguration {
    #[allow(clippy::too_many_lines)]
    #[instrument(skip_all, name = "activate")]
    async fn execute(&self, ctx: &mut Context) -> Result<(), HiveLibError> {
        let built_path = self.top_level.require().await?;

        if matches!(
            self.goal,
            // switch profile if switch or boot
            // https://github.com/NixOS/nixpkgs/blob/a2c92aa34735a04010671e3378e2aa2d109b2a72/pkgs/by-name/ni/nixos-rebuild-ng/src/nixos_rebuild/services.py#L224
            SwitchToConfigurationGoal::Switch | SwitchToConfigurationGoal::Boot
        ) {
            self.set_profile(&built_path, ctx).await?;
        }

        info!("Running switch-to-configuration {}", self.goal);

        let mut command_string = CommandStringBuilder::new(format!(
            "{}/bin/switch-to-configuration",
            built_path.to_absolute_path()
        ));
        command_string.arg(match self.goal {
            SwitchToConfigurationGoal::Switch => "switch",
            SwitchToConfigurationGoal::Boot => "boot",
            SwitchToConfigurationGoal::Test => "test",
            SwitchToConfigurationGoal::DryActivate => "dry-activate",
        });

        let child = run_command(
            &CommandArguments::new(
                command_string,
                ctx.modifiers.clone(),
                Some(ctx.name.clone()),
            )
            .execute_on_remote(self.target.clone())
            .privileged(&self.privilege_escalation_command)
            .log_stdout(),
        )
        .await?;

        let result = child.wait_till_success().await;

        match result {
            Ok(_) => {
                if !self.reboot {
                    return Ok(());
                }

                let Some(ref target) = self.target else {
                    error!("Refusing to reboot local machine!");

                    return Ok(());
                };

                warn!("Rebooting {name}!", name = ctx.name);

                let reboot = run_command(
                    &CommandArguments::new(
                        "reboot now",
                        ctx.modifiers.clone(),
                        Some(ctx.name.clone()),
                    )
                    .log_stdout()
                    .execute_on_remote(Some(target.clone()))
                    .privileged(&self.privilege_escalation_command),
                )
                .await?;

                // consume result, impossible to know if the machine failed to reboot or we
                // simply disconnected
                let _ = reboot
                    .wait_till_success()
                    .await
                    .map_err(HiveLibError::CommandError)?;

                info!("Rebooted {name}, waiting to reconnect...", name = ctx.name);

                if wait_for_ping(target, ctx).await.is_ok() {
                    return Ok(());
                }

                let target = target.0.read().await;

                error!(
                    "Failed to get regain connection to {name} via {host} after reboot.",
                    name = ctx.name,
                    host = target.get_preferred_host()?
                );

                return Err(HiveLibError::NetworkError(
                    NetworkError::HostUnreachableAfterReboot(
                        target.get_preferred_host()?.to_string(),
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
                let Some(target) = self
                    .target
                    .as_ref()
                    .filter(|_| !matches!(self.goal, SwitchToConfigurationGoal::DryActivate))
                else {
                    return Err(HiveLibError::ActivationError(
                        ActivationError::SwitchToConfigurationError(
                            self.goal,
                            ctx.name.clone(),
                            error,
                        ),
                    ));
                };

                if wait_for_ping(target, ctx).await.is_ok() {
                    return Err(HiveLibError::ActivationError(
                        ActivationError::SwitchToConfigurationError(
                            self.goal,
                            ctx.name.clone(),
                            error,
                        ),
                    ));
                }

                let target = target.0.read().await;

                error!(
                    "Failed to get regain connection to {name} via {host} after {goal} activation.",
                    name = ctx.name,
                    host = target.get_preferred_host()?,
                    goal = self.goal
                );

                return Err(HiveLibError::NetworkError(
                    NetworkError::HostUnreachableAfterReboot(
                        target.get_preferred_host()?.to_string(),
                    ),
                ));
            }
        }
    }
}
