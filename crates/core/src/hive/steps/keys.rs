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

fn get_u32_unix_mode(key: &Key) -> Result<u32, KeyError> {
    u32::from_str_radix(&key.permissions, 8).map_err(KeyError::ParseKeyPermissions)
}

pub struct SimpleLengthDelimWriter<F> {
    codec: LengthDelimitedCodec,
    write_fn: F,
}

impl<F> SimpleLengthDelimWriter<F>
where
    F: AsyncFnMut(Vec<u8>) -> Result<(), HiveLibError>,
{
    pub fn new(write_fn: F) -> Self {
        Self {
            codec: LengthDelimitedCodec::new(),
            write_fn,
        }
    }

    pub async fn send(&mut self, data: prost::bytes::Bytes) -> Result<(), HiveLibError> {
        let mut buffer = BytesMut::new();
        tokio_util::codec::Encoder::encode(&mut self.codec, data, &mut buffer)
            .map_err(HiveLibError::Encoding)?;

        (self.write_fn)(buffer.to_vec()).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use im::Vector;

    // use crate::hive::steps::keys::{Key, Keys, UploadKeyAt, process_key};
    //
    // fn new_key(upload_at: &UploadKeyAt) -> Key {
    //     Key {
    //         upload_at: upload_at.clone(),
    //         source: super::Source::String(match upload_at {
    //             UploadKeyAt::PreActivation => "pre".into(),
    //             UploadKeyAt::PostActivation => "post".into(),
    //             UploadKeyAt::NoFilter => "none".into(),
    //         }),
    //         ..Default::default()
    //     }
    // }
    //
    // #[tokio::test]
    // async fn key_filtering() {
    //     let keys = Vector::from(vec![
    //         new_key(&UploadKeyAt::PreActivation),
    //         new_key(&UploadKeyAt::PostActivation),
    //         new_key(&UploadKeyAt::PreActivation),
    //         new_key(&UploadKeyAt::PostActivation),
    //     ]);
    //
    //     for (_, buf) in (Keys {
    //         filter: crate::hive::steps::keys::UploadKeyAt::PreActivation,
    //     })
    //     .select_keys(&keys)
    //     .await
    //     .unwrap()
    //     {
    //         assert_eq!(String::from_utf8_lossy(&buf), "pre");
    //     }
    //
    //     for (_, buf) in (Keys {
    //         filter: crate::hive::steps::keys::UploadKeyAt::PostActivation,
    //     })
    //     .select_keys(&keys)
    //     .await
    //     .unwrap()
    //     {
    //         assert_eq!(String::from_utf8_lossy(&buf), "post");
    //     }
    //
    //     // test that NoFilter processes all keys.
    //     let processed_all =
    //         futures::future::join_all(keys.iter().map(async |x| process_key(x).await))
    //             .await
    //             .iter()
    //             .flatten()
    //             .cloned()
    //             .collect::<Vec<_>>();
    //     let no_filter = (Keys {
    //         filter: crate::hive::steps::keys::UploadKeyAt::NoFilter,
    //     })
    //     .select_keys(&keys)
    //     .await
    //     .unwrap()
    //     .collect::<Vec<_>>();
    //
    //     assert_eq!(processed_all, no_filter);
    // }
}
