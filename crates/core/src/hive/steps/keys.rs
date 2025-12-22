// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

use base64::Engine;
use base64::prelude::BASE64_STANDARD;
use futures::future::join_all;
use im::Vector;
use itertools::{Itertools, Position};
use owo_colors::OwoColorize;
use prost::Message;
use prost::bytes::BytesMut;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::env;
use std::fmt::Display;
use std::io::Cursor;
use std::iter::Peekable;
use std::path::PathBuf;
use std::pin::Pin;
use std::process::Stdio;
use std::str::from_utf8;
use std::vec::IntoIter;
use tokio::io::AsyncReadExt as _;
use tokio::process::Command;
use tokio::{fs::File, io::AsyncRead};
use tokio_util::codec::LengthDelimitedCodec;
use tracing::{debug, instrument};

use crate::HiveLibError;
use crate::commands::builder::CommandStringBuilder;
use crate::commands::common::push;
use crate::commands::{CommandArguments, WireCommandChip, run_command};
use crate::errors::KeyError;
use crate::hive::node::{Context, ExecuteStep, Goal, Objective, Push, SwitchToConfigurationGoal};

#[derive(Serialize, Deserialize, Clone, Debug, Eq, PartialEq, Hash)]
#[serde(tag = "t", content = "c")]
pub enum Source {
    String(String),
    Path(PathBuf),
    Command(Vec<String>),
}

#[derive(Serialize, Deserialize, Clone, Debug, Hash, Eq, PartialEq)]
pub enum UploadKeyAt {
    #[serde(rename = "pre-activation")]
    PreActivation,
    #[serde(rename = "post-activation")]
    PostActivation,
    #[serde(skip)]
    NoFilter,
}

#[derive(Serialize, Deserialize, Clone, Debug, Eq, PartialEq, Hash)]
pub struct Key {
    pub name: String,
    #[serde(rename = "destDir")]
    pub dest_dir: String,
    pub path: PathBuf,
    pub group: String,
    pub user: String,
    pub permissions: String,
    pub source: Source,
    #[serde(rename = "uploadAt")]
    pub upload_at: UploadKeyAt,
    #[serde(default)]
    pub environment: im::HashMap<String, String>,
}

impl Display for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} {} {}:{} {}",
            match self.source {
                Source::String(_) => "Literal",
                Source::Path(_) => "Path",
                Source::Command(_) => "Command",
            }
            .if_supports_color(owo_colors::Stream::Stdout, |x| x.dimmed()),
            [self.dest_dir.clone(), self.name.clone()]
                .iter()
                .collect::<PathBuf>()
                .display(),
            self.user,
            self.group,
            self.permissions,
        )
    }
}

#[cfg(test)]
impl Default for Key {
    fn default() -> Self {
        use im::HashMap;

        Self {
            name: "key".into(),
            dest_dir: "/somewhere/".into(),
            path: "key".into(),
            group: "root".into(),
            user: "root".into(),
            permissions: "0600".into(),
            source: Source::String("test key".into()),
            upload_at: UploadKeyAt::PreActivation,
            environment: HashMap::new(),
        }
    }
}

fn get_u32_permission(key: &Key) -> Result<u32, KeyError> {
    u32::from_str_radix(&key.permissions, 8).map_err(KeyError::ParseKeyPermissions)
}

async fn create_reader(key: &'_ Key) -> Result<Pin<Box<dyn AsyncRead + Send + '_>>, KeyError> {
    match &key.source {
        Source::Path(path) => Ok(Box::pin(File::open(path).await.map_err(KeyError::File)?)),
        Source::String(string) => Ok(Box::pin(Cursor::new(string))),
        Source::Command(args) => {
            let output = Command::new(args.first().ok_or(KeyError::Empty)?)
                .args(&args[1..])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .envs(key.environment.clone())
                .spawn()
                .map_err(|err| KeyError::CommandSpawnError {
                    error: err,
                    command: args.join(" "),
                    command_span: Some((0..args.first().unwrap().len()).into()),
                })?
                .wait_with_output()
                .await
                .map_err(|err| KeyError::CommandResolveError {
                    error: err,
                    command: args.join(" "),
                })?;

            if output.status.success() {
                return Ok(Box::pin(Cursor::new(output.stdout)));
            }

            Err(KeyError::CommandError(
                output.status,
                from_utf8(&output.stderr).unwrap().to_string(),
            ))
        }
    }
}

async fn process_key(key: &Key) -> Result<(wire_key_agent::keys::KeySpec, Vec<u8>), KeyError> {
    let mut reader = create_reader(key).await?;

    let mut buf = Vec::new();

    reader
        .read_to_end(&mut buf)
        .await
        .expect("failed to read into buffer");

    let destination: PathBuf = [key.dest_dir.clone(), key.name.clone()].iter().collect();

    debug!("Staging push to {}", destination.clone().display());

    Ok((
        wire_key_agent::keys::KeySpec {
            length: buf
                .len()
                .try_into()
                .expect("Failed to convert usize buf length to i32"),
            user: key.user.clone(),
            group: key.group.clone(),
            permissions: get_u32_permission(key)?,
            destination: destination.into_os_string().into_string().unwrap(),
            digest: Sha256::digest(&buf).to_vec(),
            last: false,
        },
        buf,
    ))
}

#[derive(Debug, PartialEq)]
pub struct Keys {
    pub filter: UploadKeyAt,
}
#[derive(Debug, PartialEq)]
pub struct PushKeyAgent;

impl Display for Keys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Upload key @ {:?}", self.filter)
    }
}

impl Display for PushKeyAgent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Push the key agent")
    }
}

pub struct SimpleLengthDelimWriter<F> {
    codec: LengthDelimitedCodec,
    write_fn: F,
}

impl<F> SimpleLengthDelimWriter<F>
where
    F: AsyncFnMut(Vec<u8>) -> Result<(), HiveLibError>,
{
    fn new(write_fn: F) -> Self {
        Self {
            codec: LengthDelimitedCodec::new(),
            write_fn,
        }
    }

    async fn send(&mut self, data: prost::bytes::Bytes) -> Result<(), HiveLibError> {
        let mut buffer = BytesMut::new();
        tokio_util::codec::Encoder::encode(&mut self.codec, data, &mut buffer)
            .map_err(HiveLibError::Encoding)?;

        (self.write_fn)(buffer.to_vec()).await?;
        Ok(())
    }
}

impl ExecuteStep for Keys {
    fn should_execute(&self, ctx: &Context) -> bool {
        let Objective::Apply(apply_objective) = ctx.objective else {
            return false;
        };

        if apply_objective.no_keys {
            return false;
        }

        // should execute if no filter, and the goal is keys.
        // otherwise, only execute if the goal is switch and non-nofilter
        matches!(
            (&self.filter, &apply_objective.goal),
            (UploadKeyAt::NoFilter, Goal::Keys)
                | (
                    UploadKeyAt::PreActivation | UploadKeyAt::PostActivation,
                    Goal::SwitchToConfiguration(SwitchToConfigurationGoal::Switch)
                )
        )
    }

    #[instrument(skip_all, name = "keys")]
    async fn execute(&self, ctx: &mut Context<'_>) -> Result<(), HiveLibError> {
        let agent_directory = ctx.state.key_agent_directory.as_ref().unwrap();

        let mut keys = self.select_keys(&ctx.node.keys).await?;

        if keys.peek().is_none() {
            debug!("Had no keys to push, ending KeyStep early.");
            return Ok(());
        }

        let command_string =
            CommandStringBuilder::new(format!("{agent_directory}/bin/wire-key-agent"));

        let Objective::Apply(apply_objective) = ctx.objective else {
            unreachable!()
        };

        let mut child = run_command(
            &CommandArguments::new(command_string, ctx.modifiers)
                .on_target(if apply_objective.should_apply_locally {
                    None
                } else {
                    Some(&ctx.node.target)
                })
                .elevated(ctx.node)
                .keep_stdin_open()
                .log_stdout(),
        )
        .await?;

        let mut writer = SimpleLengthDelimWriter::new(async |data| child.write_stdin(data).await);

        for (position, (mut spec, buf)) in keys.with_position() {
            if matches!(position, Position::Last | Position::Only) {
                spec.last = true;
            }

            debug!("Writing spec & buf for {:?}", spec);

            writer
                .send(BASE64_STANDARD.encode(spec.encode_to_vec()).into())
                .await?;
            writer.send(BASE64_STANDARD.encode(buf).into()).await?;
        }

        let status = child
            .wait_till_success()
            .await
            .map_err(HiveLibError::CommandError)?;

        debug!("status: {status:?}");

        Ok(())
    }
}

impl Keys {
    async fn select_keys(
        &self,
        keys: &Vector<Key>,
    ) -> Result<Peekable<IntoIter<(wire_key_agent::keys::KeySpec, std::vec::Vec<u8>)>>, HiveLibError>
    {
        let futures = keys
            .iter()
            .filter(|key| self.filter == UploadKeyAt::NoFilter || (key.upload_at == self.filter))
            .map(|key| async move {
                process_key(key)
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

impl ExecuteStep for PushKeyAgent {
    fn should_execute(&self, ctx: &Context) -> bool {
        let Objective::Apply(apply_objective) = ctx.objective else {
            return false;
        };

        if apply_objective.no_keys {
            return false;
        }

        matches!(
            &apply_objective.goal,
            Goal::Keys | Goal::SwitchToConfiguration(SwitchToConfigurationGoal::Switch)
        )
    }

    #[instrument(skip_all, name = "push_agent")]
    async fn execute(&self, ctx: &mut Context<'_>) -> Result<(), HiveLibError> {
        let arg_name = format!(
            "WIRE_KEY_AGENT_{platform}",
            platform = ctx.node.host_platform.replace('-', "_")
        );

        let agent_directory = match env::var_os(&arg_name) {
            Some(agent) => agent.into_string().unwrap(),
            None => panic!(
                "{arg_name} environment variable not set! \n
                wire was not built with the ability to deploy keys to this platform. \n
                Please create an issue: https://github.com/mrshmllow/wire/issues/new?template=bug_report.md"
            ),
        };

        let Objective::Apply(apply_objective) = ctx.objective else {
            unreachable!()
        };

        if !apply_objective.should_apply_locally {
            push(ctx, Push::Path(&agent_directory)).await?;
        }

        ctx.state.key_agent_directory = Some(agent_directory);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use im::Vector;

    use crate::hive::steps::keys::{Key, Keys, UploadKeyAt, process_key};

    fn new_key(upload_at: &UploadKeyAt) -> Key {
        Key {
            upload_at: upload_at.clone(),
            source: super::Source::String(match upload_at {
                UploadKeyAt::PreActivation => "pre".into(),
                UploadKeyAt::PostActivation => "post".into(),
                UploadKeyAt::NoFilter => "none".into(),
            }),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn key_filtering() {
        let keys = Vector::from(vec![
            new_key(&UploadKeyAt::PreActivation),
            new_key(&UploadKeyAt::PostActivation),
            new_key(&UploadKeyAt::PreActivation),
            new_key(&UploadKeyAt::PostActivation),
        ]);

        for (_, buf) in (Keys {
            filter: crate::hive::steps::keys::UploadKeyAt::PreActivation,
        })
        .select_keys(&keys)
        .await
        .unwrap()
        {
            assert_eq!(String::from_utf8_lossy(&buf), "pre");
        }

        for (_, buf) in (Keys {
            filter: crate::hive::steps::keys::UploadKeyAt::PostActivation,
        })
        .select_keys(&keys)
        .await
        .unwrap()
        {
            assert_eq!(String::from_utf8_lossy(&buf), "post");
        }

        // test that NoFilter processes all keys.
        let processed_all =
            futures::future::join_all(keys.iter().map(async |x| process_key(x).await))
                .await
                .iter()
                .flatten()
                .cloned()
                .collect::<Vec<_>>();
        let no_filter = (Keys {
            filter: crate::hive::steps::keys::UploadKeyAt::NoFilter,
        })
        .select_keys(&keys)
        .await
        .unwrap()
        .collect::<Vec<_>>();

        assert_eq!(processed_all, no_filter);
    }
}
