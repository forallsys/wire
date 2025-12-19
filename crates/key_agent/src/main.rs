// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

#![deny(clippy::pedantic)]
use base64::Engine;
use base64::prelude::BASE64_STANDARD;
use futures_util::stream::StreamExt;
use nix::unistd::{Group, User};
use prost::Message;
use prost::bytes::Bytes;
use sha2::{Digest, Sha256};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::fs::chown;
use std::path::{Path, PathBuf};
use tokio::fs::File;
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
        "{} {}:{} {}",
        spec.destination, spec.user, spec.group, spec.permissions
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
        create_path(&path)?;

        let mut file = File::create(path).await?;
        let mut permissions = file.metadata().await?.permissions();

        permissions.set_mode(spec.permissions);
        file.set_permissions(permissions).await?;

        let user = User::from_name(&spec.user)?;
        let group = Group::from_name(&spec.group)?;

        chown(
            spec.destination,
            // Default uid/gid to 0. This is then wrapped around an Option again for
            // the function.
            Some(user.map_or(0, |user| user.uid.into())),
            Some(group.map_or(0, |group| group.gid.into())),
        )?;

        file.write_all(&key_bytes).await?;

        // last key, goobye
        if spec.last {
            break;
        }
    }

    Ok(())
}
