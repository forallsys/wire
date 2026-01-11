use base64::Engine;
use wire_core::commands::ChildOutputMode;
use prost::Message;
use futures_util::future::join_all;
use itertools::Itertools;
use base64::prelude::BASE64_STANDARD;
use itertools::Position;
use enum_dispatch::enum_dispatch;
use secrecy::{ExposeSecret, SecretSlice};
use wire_core::commands::builder::CommandStringBuilder;
use wire_core::hive::node::Target;
use wire_core::hive::steps::keys::SimpleLengthDelimWriter;
use wire_core::commands::{CommandArguments, Either, WireCommandChip, run_command, run_command_with_env};
use wire_key_agent::keys::KeySpec;
use std::{env, sync::Arc};
use tracing::{Level, debug, event, info};
use wire_core::{
    commands::common::push,
    errors::HiveLibError,
    hive::node::{Context, Push, SwitchToConfigurationGoal},
};
use wire_keys::Key;

pub mod activate;

#[enum_dispatch]
pub trait ExecuteStep {
    #[allow(async_fn_in_trait)]
    async fn execute(&self, ctx: &mut Context) -> Result<(), HiveLibError>;
}

pub struct Ping;
pub struct PushKeyAgent {
    pub substitute_on_destination: bool,
    pub host_platform: Arc<str>,
    pub target: Arc<Target>,
}
pub struct Keys {
    pub keys: Vec<Arc<Key>>,
    pub target: Option<Arc<Target>>,
    pub privilege_escalation_command: Vec<Arc<str>>
}
pub struct Evaluate;
pub struct PushEvaluatedOutput {
    pub substitute_on_destination: bool,
    pub target: Arc<Target>,
}
pub struct Build {
    pub target: Option<Arc<Target>>,
}
pub struct PushBuildOutput {
    pub substitute_on_destination: bool,
    pub target: Arc<Target>,
}

impl ExecuteStep for Ping {
    async fn execute(&self, ctx: &mut Context<'_>) -> Result<(), HiveLibError> {
        loop {
            todo!()
            // event!(
            //     Level::INFO,
            //     status = "attempting",
            //     host = ctx.node.target.get_preferred_host()?.to_string()
            // );
            //
            // if ctx.node.ping(ctx.modifiers).await.is_ok() {
            //     event!(
            //         Level::INFO,
            //         status = "success",
            //         host = ctx.node.target.get_preferred_host()?.to_string()
            //     );
            //     return Ok(());
            // }
            //
            // // ? will take us out if we ran out of hosts
            // event!(
            //     Level::WARN,
            //     status = "failed to ping",
            //     host = ctx.node.target.get_preferred_host()?.to_string()
            // );
            // ctx.node.target.host_failed();
        }
    }
}

impl ExecuteStep for PushKeyAgent {
    async fn execute(&self, ctx: &mut Context<'_>) -> Result<(), HiveLibError> {
        let arg_name = format!(
            "WIRE_KEY_AGENT_{platform}",
            platform = self.host_platform.replace('-', "_")
        );

        let agent_directory = match env::var_os(&arg_name) {
            Some(agent) => agent.into_string().unwrap(),
            None => panic!(
                "{arg_name} environment variable not set! \n
                wire was not built with the ability to deploy keys to this platform. \n
                Please create an issue: https://github.com/forallsys/wire/issues/new?template=bug_report.md"
            ),
        };

        push(ctx, &self.target, Push::Path(&agent_directory), self.substitute_on_destination).await?;

        ctx.state.key_agent_directory = Some(agent_directory);

        Ok(())
    }
}

impl Keys {
    async fn select_keys(&self) -> Result<impl Iterator<Item = (KeySpec, SecretSlice<u8>)>, HiveLibError>{
        let futures = self.keys
            .iter()
            .map(|key| async move {
                key.read()
                    .await
                    .map_err(|err| HiveLibError::KeyError(key.name.clone(), err))
            });

        Ok(join_all(futures)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, HiveLibError>>()?
            .into_iter()
            .peekable())
    }
}

impl ExecuteStep for Keys {
    async fn execute(&self, ctx: &mut Context<'_>) -> Result<(), HiveLibError> {
        let agent_directory = ctx.state.key_agent_directory.as_ref().unwrap();

        let command_string =
            CommandStringBuilder::new(format!("{agent_directory}/bin/wire-key-agent"));

        let mut child = run_command(
            &CommandArguments::new(command_string, ctx.modifiers)
                .execute_on_remote(self.target.as_deref())
                .privileged(&self.privilege_escalation_command)
                .keep_stdin_open()
                .log_stdout(),
        )
        .await?;

        let mut writer = SimpleLengthDelimWriter::new(async |data| child.write_stdin(data).await);

        let keys = self.select_keys().await?;

        for (position, (mut spec, buf)) in keys.with_position() {
            if matches!(position, Position::Last | Position::Only) {
                spec.last = true;
            }

            debug!("Writing spec & buf for {:?}", spec);

            writer
                .send(BASE64_STANDARD.encode(spec.encode_to_vec()).into())
                .await?;
            writer.send(BASE64_STANDARD.encode(buf.expose_secret()).into()).await?;
        }

        let status = child
            .wait_till_success()
            .await
            .map_err(HiveLibError::CommandError)?;

        debug!("status: {status:?}");

        Ok(())
    }
}

impl ExecuteStep for Evaluate {
    async fn execute(&self, ctx: &mut Context<'_>) -> Result<(), HiveLibError> {
        let rx = ctx.state.evaluation_rx.take().unwrap();

        ctx.state.evaluation = Some(rx.await.unwrap()?);

        Ok(())
    }
}

impl ExecuteStep for PushEvaluatedOutput {
    async fn execute(&self, ctx: &mut Context<'_>) -> Result<(), HiveLibError> {
        let top_level = ctx.state.evaluation.as_ref().unwrap();

        push(ctx, &self.target, Push::Derivation(top_level), self.substitute_on_destination).await?;

        Ok(())
    }
}

impl ExecuteStep for Build {
    async fn execute(&self, ctx: &mut Context<'_>) -> Result<(), HiveLibError> {
        let top_level = ctx.state.evaluation.as_ref().unwrap();

        let mut command_string = CommandStringBuilder::nix();
        command_string.args(&[
            "--extra-experimental-features",
            "nix-command",
            "build",
            "--print-build-logs",
            "--no-link",
            "--print-out-paths",
        ]);
        command_string.arg(top_level.to_string());

        let status = run_command_with_env(
            &CommandArguments::new(command_string, ctx.modifiers)
                .execute_on_remote(self.target.as_deref())
                .mode(ChildOutputMode::Nix)
                .log_stdout(),
            std::collections::HashMap::new(),
        )
        .await?
        .wait_till_success()
        .await
        .map_err(|source| HiveLibError::NixBuildError {
            name: ctx.name.clone(),
            source,
        })?;

        let stdout = match status {
            Either::Left((_, stdout)) | Either::Right((_, stdout)) => stdout,
        };

        info!("Built output: {stdout:?}");

        // print built path to stdout
        println!("{stdout}");

        ctx.state.build = Some(stdout);

        Ok(())
    }
}

impl ExecuteStep for PushBuildOutput {
    async fn execute(&self, ctx: &mut Context<'_>) -> Result<(), HiveLibError> {
        let built_path = ctx.state.build.as_ref().unwrap();

        push(ctx, &self.target, Push::Path(built_path), self.substitute_on_destination).await?;

        Ok(())
    }
}
