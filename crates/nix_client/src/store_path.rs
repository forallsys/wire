use miette::Diagnostic;
use nix_compat::wire::ser::NixSerialize;
use serde::Deserialize;
use std::hash::Hash;
use thiserror::Error;

#[derive(Debug, Diagnostic, Error)]
#[diagnostic(code(wire::SnixStorePath))]
#[error("Failed to parse store path {path:?}")]
pub struct StorePathError {
    path: String,
    #[source]
    error: nix_compat::store_path::Error,
}

/// This type exists to restrict `StorePath` usage to only methods that deal with
/// absolute paths. By default, the `StorePath` type implements Display that
/// does not include `/nix/store/` can introduce many hard to catch bugs.
///
///
/// If <https://github.com/rust-lang/rust-clippy/issues/8581>
/// is ever closed, this can be dropped from the codebase.
#[derive(Clone)]
#[allow(clippy::disallowed_types)]
pub struct SafeStorePath<S>(pub nix_compat::store_path::StorePath<S>);

#[allow(clippy::disallowed_types)]
impl<S> SafeStorePath<S> {
    pub fn from_absolute_path<'a>(s: &'a [u8]) -> Result<SafeStorePath<S>, StorePathError>
    where
        S: From<&'a str> + AsRef<str>,
    {
        Ok(Self(
            nix_compat::store_path::StorePath::from_absolute_path(s).map_err(|error| {
                StorePathError {
                    path: String::from_utf8_lossy(s).to_string(),
                    error,
                }
            })?,
        ))
    }

    pub fn into_inner(self) -> nix_compat::store_path::StorePath<S> {
        self.0
    }

    pub fn from_name_and_digest<'a>(name: &'a str, digest: &[u8]) -> Result<Self, StorePathError>
    where
        S: From<&'a str> + AsRef<str>,
    {
        Ok(Self(
            nix_compat::store_path::StorePath::from_name_and_digest(name, digest).map_err(
                |error| StorePathError {
                    path: format!("raw name & digest: {digest:?}-{name:?}"),
                    error,
                },
            )?,
        ))
    }

    pub fn to_absolute_path(&self) -> String
    where
        S: AsRef<str>,
    {
        self.0.to_absolute_path()
    }

    pub fn digest(&self) -> &[u8; nix_compat::store_path::DIGEST_SIZE]
    where
        S: AsRef<str>,
    {
        self.0.digest()
    }

    pub fn name(&self) -> &S
    where
        S: AsRef<str>,
    {
        self.0.name()
    }
}

#[allow(clippy::disallowed_types)]
impl<'de, S> Deserialize<'de> for SafeStorePath<S>
where
    nix_compat::store_path::StorePath<S>: Deserialize<'de>,
    S: AsRef<str>,
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

impl<S> Hash for SafeStorePath<S>
where
    S: AsRef<str>,
{
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl<S> Eq for SafeStorePath<S> where S: AsRef<str> {}

impl<S> NixSerialize for SafeStorePath<S>
where
    S: AsRef<str>,
{
    fn serialize<W>(&self, writer: &mut W) -> impl Future<Output = Result<(), W::Error>> + Send
    where
        W: nix_compat::wire::ser::NixWrite,
    {
        self.0.serialize(writer)
    }
}

impl<S> NixSerialize for &SafeStorePath<S>
where
    S: AsRef<str>,
{
    fn serialize<W>(&self, writer: &mut W) -> impl Future<Output = Result<(), W::Error>> + Send
    where
        W: nix_compat::wire::ser::NixWrite,
    {
        self.0.serialize(writer)
    }
}

impl nix_compat::wire::de::NixDeserialize for SafeStorePath<String> {
    async fn try_deserialize<R>(reader: &mut R) -> Result<Option<Self>, R::Error>
    where
        R: ?Sized + nix_compat::wire::de::NixRead + Send,
    {
        if let Some(store_path) = reader.try_read_value().await? {
            Ok(Some(SafeStorePath(store_path)))
        } else {
            Ok(None)
        }
    }
}

#[allow(clippy::disallowed_types)]
impl<S> From<nix_compat::store_path::StorePath<S>> for SafeStorePath<S> {
    fn from(value: nix_compat::store_path::StorePath<S>) -> Self {
        SafeStorePath(value)
    }
}

#[allow(clippy::disallowed_types)]
impl<S> From<SafeStorePath<S>> for nix_compat::store_path::StorePath<S> {
    fn from(value: SafeStorePath<S>) -> nix_compat::store_path::StorePath<S> {
        value.into_inner()
    }
}

impl<S> std::fmt::Debug for SafeStorePath<S>
where
    S: AsRef<str>,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0.to_absolute_path())
    }
}
