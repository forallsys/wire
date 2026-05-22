// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

#![feature(iter_intersperse)]
#![feature(sync_nonpoison)]
#![feature(nonpoison_mutex)]

use std::{io::IsTerminal, sync::LazyLock};

use serde::Deserialize;
use tokio::sync::{AcquireError, Semaphore, SemaphorePermit, mpsc::UnboundedSender, oneshot};

use crate::{
    errors::HiveLibError,
    hive::node::Name,
    status::{UI_SENDER, UiMessage},
};

pub mod cache;
pub mod commands;
pub mod hive;
pub mod status;

#[cfg(test)]
mod test_macros;

#[cfg(test)]
mod test_support;

pub mod errors;

#[derive(Clone, Debug, Copy, Default)]
pub enum StrictHostKeyChecking {
    /// do not accept new host. dangerous!
    No,

    /// accept-new, default
    #[default]
    AcceptNew,
}

#[derive(Debug, Clone, Copy)]
pub struct SubCommandModifiers {
    pub show_trace: bool,
    pub non_interactive: bool,
    pub ssh_accept_host: StrictHostKeyChecking,
    pub ssh_verbosity: usize,
    pub print_build_logs: bool,
}

impl Default for SubCommandModifiers {
    fn default() -> Self {
        SubCommandModifiers {
            show_trace: false,
            non_interactive: !std::io::stdin().is_terminal(),
            ssh_accept_host: StrictHostKeyChecking::default(),
            ssh_verbosity: 0,
            print_build_logs: false,
        }
    }
}

pub enum EvalGoal<'a> {
    Inspect,
    Names,
    GetTopLevel(&'a Name),
}

pub static STDIN_CLOBBER_LOCK: LazyLock<Semaphore> = LazyLock::new(|| Semaphore::new(1));

/// `SemaphorePermit` that sends a `UiMessage::Release` on drop
pub struct ClobberGuard<'a>(
    #[allow(unused)] SemaphorePermit<'a>,
    Option<&'a UnboundedSender<UiMessage>>,
);

impl Drop for ClobberGuard<'_> {
    fn drop(&mut self) {
        if let Some(tx) = self.1 {
            let _ = tx.send(UiMessage::Release);
        }
    }
}

pub async fn acquire_stdin_lock<'a>() -> Result<ClobberGuard<'a>, AcquireError> {
    let result = STDIN_CLOBBER_LOCK.acquire().await?;
    let (sender, rx) = oneshot::channel();
    let tx = UI_SENDER.get();

    if let Some(tx) = tx {
        let _ = tx.send(UiMessage::Takeover(sender));

        // wait until takeover is confirmed
        let _ = rx.await;
    }

    let result = ClobberGuard(result, tx);

    Ok(result)
}

/// This type exists to restrict `StorePath` usage to only methods that deal with
/// absolute paths. By default, the `StorePath` type implements Display that
/// does not include `/nix/store/` can introduce many hard to catch bugs.
///
///
/// If <https://github.com/rust-lang/rust-clippy/issues/8581>
/// is ever closed, this can be dropped from the codebase.
#[derive(Debug, Clone)]
#[allow(clippy::disallowed_types)]
pub struct SafeStorePath<S>(nix_compat::store_path::StorePath<S>);

#[allow(clippy::disallowed_types)]
impl<S> SafeStorePath<S>
where
    S: AsRef<str>,
{
    pub fn from_absolute_path<'a>(s: &'a [u8]) -> Result<SafeStorePath<S>, HiveLibError>
    where
        S: From<&'a str>,
    {
        Ok(Self(
            nix_compat::store_path::StorePath::from_absolute_path(s).map_err(|error| {
                HiveLibError::StorePath {
                    path: String::from_utf8_lossy(s).to_string(),
                    error,
                }
            })?,
        ))
    }

    pub fn from_name_and_digest<'a>(name: &'a str, digest: &[u8]) -> Result<Self, HiveLibError>
    where
        S: From<&'a str>,
    {
        Ok(Self(
            nix_compat::store_path::StorePath::from_name_and_digest(name, digest).map_err(
                |error| HiveLibError::StorePath {
                    path: format!("raw name & digest: {digest:?}-{name:?}"),
                    error,
                },
            )?,
        ))
    }

    pub fn into_inner(self) -> nix_compat::store_path::StorePath<S> {
        self.0
    }

    pub fn to_absolute_path(&self) -> String {
        self.0.to_absolute_path()
    }

    pub fn digest(&self) -> &[u8; nix_compat::store_path::DIGEST_SIZE] {
        self.0.digest()
    }

    pub fn name(&self) -> &S {
        self.0.name()
    }
}

#[allow(clippy::disallowed_types)]
impl<'de, S> Deserialize<'de> for SafeStorePath<S>
where
    nix_compat::store_path::StorePath<S>: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(SafeStorePath(
            nix_compat::store_path::StorePath::deserialize(deserializer)?,
        ))
    }
}

impl<S> PartialEq for SafeStorePath<S>
where
    S: AsRef<str>,
{
    fn eq(&self, other: &Self) -> bool {
        self.0.eq(&other.0)
    }
}

impl<S> Eq for SafeStorePath<S> where S: AsRef<str> {}
