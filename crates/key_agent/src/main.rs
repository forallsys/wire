// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

#![deny(clippy::pedantic)]
use anyhow::Context;
use base64::Engine;
use base64::prelude::BASE64_STANDARD;
use futures_util::stream::StreamExt;
use nix::sys::stat::fchmod;
use nix::unistd::fchown;
use nix::unistd::{Group, User};
use prost::Message;
use prost::bytes::Bytes;
use sha2::{Digest, Sha256};
use std::os::fd::AsFd;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use tokio_util::codec::{FramedRead, LengthDelimitedCodec};
use wire_key_agent::keys::KeySpec;

fn create_path(key_path: &Path) -> Result<(), anyhow::Error> {
    let prefix = key_path.parent().unwrap();
    std::fs::create_dir_all(prefix)?;

    Ok(())
}

fn pretty_keyspec(spec: &KeySpec) -> String {
    format!(
        "{} {}:{} {:o}",
        spec.destination, spec.user, spec.group, spec.unix_mode
    )
}

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    let stdin = tokio::io::stdin();

    let mut framed = FramedRead::new(stdin, LengthDelimitedCodec::new());

    while let Some(spec_bytes) = framed.next().await {
        let spec_bytes = Bytes::from(BASE64_STANDARD.decode(spec_bytes?)?);
        let spec = KeySpec::decode(spec_bytes)?;

        let key_bytes = BASE64_STANDARD.decode(
            framed
                .next()
                .await
                .expect("expected key_bytes to come after spec_bytes")?,
        )?;

        let digest = Sha256::digest(&key_bytes).to_vec();

        println!(
            "Writing {}, {:?} bytes of data",
            pretty_keyspec(&spec),
            key_bytes.len()
        );

        if digest != spec.digest {
            return Err(anyhow::anyhow!(
                "digest of {spec:?} did not match {digest:?}! Please create an issue!"
            ));
        }

        let path = PathBuf::from(&spec.destination);
        create_path(&path).context("creating directory for key")?;

        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            // only applies if the file is created
            .mode(spec.unix_mode)
            .custom_flags(nix::libc::O_NOFOLLOW)
            .open(&path)
            .await
            .context("opening file")?;

        // enforce permission on existing files
        let mode = nix::sys::stat::Mode::from_bits(spec.unix_mode)
            .with_context(|| format!("failed to create unix mode: {:o}", spec.unix_mode))?;

        fchmod(file.as_fd(), mode)
            .with_context(|| format!("setting permissions of fd to {:o}", spec.unix_mode))?;

        // Default uid/gid to 0. This is then wrapped around an Option again for
        // the function.
        let user = Some(
            User::from_name(&spec.user)
                .context("obtaining user")?
                .map_or(
                    {
                        println!("warning: defaulting uid to `0`");

                        0.into()
                    },
                    |user| user.uid.into(),
                ),
        );
        let group = Some(
            Group::from_name(&spec.group)
                .context("obtaining group")?
                .map_or(
                    {
                        println!("warning: defaulting gid to `0`");

                        0.into()
                    },
                    |group| group.gid,
                ),
        );

        // set permission on new files
        fchown(&file, user, group)
            .with_context(|| format!("setting ownership of fd to {user:?}, {group:?}"))?;

        file.write_all(&key_bytes).await.context("writing to fd")?;

        // last key, goobye
        if spec.last {
            break;
        }
    }

    Ok(())
}
