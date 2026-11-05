use miette::Diagnostic;
use nix_compat::wire::ser::NixSerialize;
use serde::Deserialize;
use std::hash::Hash;
use thiserror::Error;

#[derive(Debug, Diagnostic, Error)]
pub enum StorePathError {
    #[diagnostic(code(wire::StorePath::Parse))]
    #[error("Failed to parse store path {path:?}")]
    PathError {
        path: String,
        #[source]
        error: nix_compat::store_path::ParseStorePathError,
    },

    #[diagnostic(code(wire::StorePath::Name))]
    #[error("Failed to parse store path name {name:?}")]
    NameError {
        name: String,
        #[source]
        error: nix_compat::store_path::ParseStorePathNameError,
    },
}

/// A restricted `StorePath`.
///
/// This type exists to restrict `StorePath` usage to only methods that deal with
/// absolute paths. By default, the `StorePath` type implements Display that
/// does not include `/nix/store/` can introduce many hard to catch bugs.
///
/// If <https://github.com/rust-lang/rust-clippy/issues/8581>
/// is ever closed, this can be dropped from the codebase.
#[derive(Clone, Eq, Hash, PartialEq, Debug)]
#[allow(clippy::disallowed_types)]
pub struct SafeStorePath(pub nix_compat::store_path::StorePath);

#[allow(clippy::disallowed_types)]
impl SafeStorePath {
    pub fn from_absolute_path(s: &[u8]) -> Result<Self, StorePathError> {
        Ok(Self(
            nix_compat::store_path::StorePath::from_absolute_path(s).map_err(|error| {
                StorePathError::PathError {
                    path: String::from_utf8_lossy(s).to_string(),
                    error,
                }
            })?,
        ))
    }

    #[must_use]
    pub fn into_inner(self) -> nix_compat::store_path::StorePath {
        self.0
    }

    pub fn from_name_and_digest(name: &str, digest: &[u8]) -> Result<Self, StorePathError> {
        let name = nix_compat::store_path::StorePathName::from_str(name).map_err(|x| {
            StorePathError::NameError {
                name: name.to_string(),
                error: x,
            }
        })?;

        Ok(Self(
            nix_compat::store_path::StorePath::from_name_and_digest(name, digest).map_err(
                |error| StorePathError::PathError {
                    path: format!("raw name & digest: {digest:?}-{name:?}"),
                    error,
                },
            )?,
        ))
    }

    #[must_use]
    pub fn to_absolute_path(&self) -> String {
        self.0.to_absolute_path()
    }

    #[must_use]
    pub fn digest(&self) -> &[u8; nix_compat::store_path::DIGEST_SIZE] {
        self.0.digest()
    }

    #[must_use]
    pub fn name(&self) -> &nix_compat::store_path::StorePathName {
        self.0.name()
    }
}

#[allow(clippy::disallowed_types)]
impl<'de> Deserialize<'de> for SafeStorePath
where
    nix_compat::store_path::StorePath: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(Self(nix_compat::store_path::StorePath::deserialize(
            deserializer,
        )?))
    }
}

impl NixSerialize for SafeStorePath {
    fn serialize<W>(&self, writer: &mut W) -> impl Future<Output = Result<(), W::Error>> + Send
    where
        W: nix_compat::wire::ser::NixWrite,
    {
        self.0.serialize(writer)
    }
}

impl NixSerialize for &SafeStorePath {
    fn serialize<W>(&self, writer: &mut W) -> impl Future<Output = Result<(), W::Error>> + Send
    where
        W: nix_compat::wire::ser::NixWrite,
    {
        self.0.serialize(writer)
    }
}

impl nix_compat::wire::de::NixDeserialize for SafeStorePath {
    async fn try_deserialize<R>(reader: &mut R) -> Result<Option<Self>, R::Error>
    where
        R: ?Sized + nix_compat::wire::de::NixRead + Send,
    {
        reader
            .try_read_value()
            .await?
            .map_or_else(|| Ok(None), |store_path| Ok(Some(Self(store_path))))
    }
}

#[allow(clippy::disallowed_types)]
impl From<nix_compat::store_path::StorePath> for SafeStorePath {
    fn from(value: nix_compat::store_path::StorePath) -> Self {
        Self(value)
    }
}

#[allow(clippy::disallowed_types)]
impl From<SafeStorePath> for nix_compat::store_path::StorePath {
    fn from(value: SafeStorePath) -> Self {
        value.into_inner()
    }
}
