use std::sync::Arc;

use tracing::{error, info, warn};
use wire_core::{commands::{ChildOutputMode, CommandArguments, WireCommandChip, builder::CommandStringBuilder, run_command}, errors::{ActivationError, HiveLibError, NetworkError}, hive::node::{Context, SwitchToConfigurationGoal, Target}};

use crate::ExecuteStep;

pub struct SwitchToConfiguration {
    pub goal: SwitchToConfigurationGoal,
    pub reboot: bool,
    pub target: Option<Arc<Target>>,
    pub privilege_escalation_command: Arc<Vec<Arc<str>>>
}

impl SwitchToConfiguration {
    async fn set_profile(
        &self,
        ctx: &Context,
        built_path: &String,
    ) -> Result<(), HiveLibError> {
        info!("Setting profiles in anticipation for switch-to-configuration {}", self.goal);

        let mut command_string = CommandStringBuilder::new("nix-env");
        command_string.args(&["-p", "/nix/var/nix/profiles/system", "--set"]);
        command_string.arg(built_path);

        let child = run_command(
            &CommandArguments::new(command_string, ctx.modifiers)
                .mode(ChildOutputMode::Nix)
                .execute_on_remote(self.target.as_deref())
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

    async fn wait_for_ping(&self, _ctx: &Context) -> Result<(), HiveLibError> {
        todo!()
        // let host = ctx.node.target.get_preferred_host()?;
        // let mut result = ctx.node.ping(ctx.modifiers).await;
        //
        // for num in 0..2 {
        //     warn!("Trying to ping {host} (attempt {}/3)", num + 1);
        //
        //     result = ctx.node.ping(ctx.modifiers).await;
        //
        //     if result.is_ok() {
        //         info!("Regained connection to {} via {host}", ctx.name);
        //
        //         break;
        //     }
        // }
        //
        // result
    }
}

impl ExecuteStep for SwitchToConfiguration {
    async fn execute(&self, ctx: &mut Context) -> Result<(), HiveLibError> {
        let built_path = ctx.state.build.as_ref().unwrap();

        if matches!(
            self.goal,
            // switch profile if switch or boot
            // https://github.com/NixOS/nixpkgs/blob/a2c92aa34735a04010671e3378e2aa2d109b2a72/pkgs/by-name/ni/nixos-rebuild-ng/src/nixos_rebuild/services.py#L224
            SwitchToConfigurationGoal::Switch | SwitchToConfigurationGoal::Boot
        ) {
            self.set_profile(ctx, built_path).await?;
        }

        info!("Running switch-to-configuration {}", self.goal);

        let mut command_string =
            CommandStringBuilder::new(format!("{built_path}/bin/switch-to-configuration"));
        command_string.arg(match self.goal {
            SwitchToConfigurationGoal::Switch => "switch",
            SwitchToConfigurationGoal::Boot => "boot",
            SwitchToConfigurationGoal::Test => "test",
            SwitchToConfigurationGoal::DryActivate => "dry-activate",
        });

        let child = run_command(
            &CommandArguments::new(command_string, ctx.modifiers)
                .execute_on_remote(self.target.as_deref())
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
                    &CommandArguments::new("reboot now", ctx.modifiers)
                        .log_stdout()
                        .execute_on_remote(self.target.as_deref())
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

                if self.wait_for_ping(ctx).await.is_ok() {
                    return Ok(());
                }

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
                let Some(target) = self.target.clone().filter(|_| !matches!(self.goal, SwitchToConfigurationGoal::DryActivate)) else {
                    return Err(HiveLibError::ActivationError(
                        ActivationError::SwitchToConfigurationError(self.goal, ctx.name.clone(), error),
                    ));
                };

                if self.wait_for_ping(ctx).await.is_ok() {
                    return Err(HiveLibError::ActivationError(
                        ActivationError::SwitchToConfigurationError(self.goal, ctx.name.clone(), error),
                    ));
                }

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

