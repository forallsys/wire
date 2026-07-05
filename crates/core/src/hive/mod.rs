// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

use itertools::Itertools;
use nix_compat::flakeref::FlakeRef;
use nix_compat::nixhash::NixHash;
use node::{Name, Node};
use owo_colors::{OwoColorize, Stream};
use semver::{Version, VersionReq};
use serde::de::Error;
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fmt::Display;
use std::fs;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{Arc, LazyLock};
use tracing::{debug, info, instrument};

use crate::cache::InspectionCache;
use crate::commands::builder::CommandStringBuilder;
use crate::commands::common::evaluate_hive_attribute;
use crate::commands::{CommandArguments, Either, WireCommandChip, run_command};
use crate::errors::{HiveInitialisationError, HiveLocationError};
use crate::{EvalGoal, HiveLibError, SafeStorePath, SubCommandModifiers};
pub mod executor;
pub mod node;
pub mod plan;
pub mod steps;

#[derive(Debug, Eq, PartialEq)]
pub enum SchemaVersion {
    Semver(semver::Version),
    DeprecatedInteger(u64),
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct Hive {
    pub nodes: HashMap<Name, Node>,

    #[serde(deserialize_with = "check_schema_version", rename = "_schema")]
    pub schema: SchemaVersion,
}

impl Serialize for SchemaVersion {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Semver(version) => serializer.serialize_str(&version.to_string()),
            Self::DeprecatedInteger(integer) => serializer.serialize_u64(*integer),
        }
    }
}

fn check_schema_version<'de, D: Deserializer<'de>>(d: D) -> Result<SchemaVersion, D::Error> {
    let value = serde_json::Value::deserialize(d)?;

    if value.is_number() {
        let number = value.as_u64().ok_or(D::Error::custom(
            "failed to read deprecated integer schema version into an u64",
        ))?;

        if number != Hive::DEPRECATED_SCHEMA_VERSION {
            return Err(D::Error::custom(format!(
                "your `makeHive` function can't be read (schema verison {number:?}, required {:?}). please upgrade it or downgrade this binary",
                Hive::DEPRECATED_SCHEMA_VERSION
            )));
        }

        return Ok(SchemaVersion::DeprecatedInteger(number));
    }

    let semver_string = value.as_str().ok_or(D::Error::custom(
        "failed to read schema version into a &str. this is likely a bug you should report",
    ))?;

    let version = Version::parse(semver_string).map_err(|err| {
        D::Error::custom(format!(
            "failed to parse schema semver. this is likely a bug you should report: {err:?}"
        ))
    })?;

    if !Hive::SCHEMA_VERSION_SEMVER.matches(&version) {
        return Err(D::Error::custom(format!(
            "your `makeHive`'s version ({version}) did not match {}. please upgrade it or downgrade this binary",
            *Hive::SCHEMA_VERSION_SEMVER
        )));
    }

    Ok(SchemaVersion::Semver(
        Version::parse(semver_string).map_err(|err| {
            D::Error::custom(format!(
                "failed to parse schema semver. this is likely a bug you should report: {err:?}"
            ))
        })?,
    ))
}

impl Hive {
    /// The schema version that was previously used before the semver schema
    /// versions where implemented
    pub const DEPRECATED_SCHEMA_VERSION: u64 = 1;

    /// Semver version requirement for schemas that this wire binary can read.
    pub const SCHEMA_VERSION_SEMVER: LazyLock<VersionReq> = LazyLock::new(|| {
        VersionReq::parse("^1.0.0").expect("hive version requirement failed to parse")
    });

    pub const SCHEMA_VERSION_STRING: LazyLock<String> =
        LazyLock::new(|| Hive::SCHEMA_VERSION_SEMVER.to_string());

    #[instrument(skip_all, name = "eval_hive")]
    pub async fn new_from_path(
        location: &HiveLocation,
        cache: Arc<Option<InspectionCache>>,
        modifiers: SubCommandModifiers,
    ) -> Result<Self, HiveLibError> {
        info!("evaluating hive {location:?}");

        if let Some(ref cache) = *cache
            && let HiveLocation::Flake { prefetch, .. } = location
            && let Some(hive) = cache.get_hive(prefetch).await
        {
            return Ok(hive);
        }

        let output = evaluate_hive_attribute(location, &EvalGoal::Inspect, modifiers).await?;

        let hive: Self = serde_json::from_str(&output).map_err(|err| {
            HiveLibError::HiveInitialisationError(HiveInitialisationError::ParseEvaluateError(err))
        })?;

        if let Some(ref cache) = *cache
            && let HiveLocation::Flake { prefetch, .. } = location
        {
            cache.store_hive(prefetch, &output).await;
        }

        Ok(hive)
    }

    /// # Errors
    ///
    /// Returns an error if a node in nodes does not exist in the hive.
    pub fn force_always_local(&mut self, nodes: Vec<String>) -> Result<(), HiveLibError> {
        for node in nodes {
            info!("Forcing a local build for {node}");

            self.nodes
                .get_mut(&Name(Arc::from(node.as_str())))
                .ok_or(HiveLibError::HiveInitialisationError(
                    HiveInitialisationError::NodeDoesNotExist(node.clone()),
                ))?
                .build_remotely = false;
        }

        Ok(())
    }
}

impl Display for Hive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (name, node) in &self.nodes {
            writeln!(
                f,
                "Node {} {}:\n",
                name.bold(),
                format!("({})", node.host_platform)
                    .italic()
                    .if_supports_color(Stream::Stdout, |x| x.dimmed()),
            )?;

            if !node.tags.is_empty() {
                write!(f, " > {}", "Tags:".bold())?;
                writeln!(f, " {:?}", node.tags)?;
            }

            write!(f, " > {}", "Connection:".bold())?;
            writeln!(f, " {{{}}}", node.target)?;

            write!(
                f,
                " > {} {}{}",
                "Build remotely".bold(),
                "`deployment.buildOnTarget`"
                    .if_supports_color(Stream::Stdout, |x| x.dimmed())
                    .italic(),
                ":".bold()
            )?;
            writeln!(f, " {}", node.build_remotely)?;

            write!(
                f,
                " > {} {}{}",
                "Local apply allowed".bold(),
                "`deployment.allowLocalDeployment`"
                    .if_supports_color(Stream::Stdout, |x| x.dimmed())
                    .italic(),
                ":".bold()
            )?;
            writeln!(f, " {}", node.allow_local_deployment)?;

            if !node.keys.is_empty() {
                write!(f, " > {}", "Keys:".bold())?;
                writeln!(f, " {} key(s)", node.keys.len())?;

                for key in &node.keys {
                    writeln!(f, "    > {key}")?;
                }
            }

            writeln!(f)?;
        }

        let total_keys = self
            .nodes
            .values()
            .flat_map(|node| node.keys.iter())
            .count();
        let distinct_keys = self
            .nodes
            .values()
            .flat_map(|node| node.keys.iter())
            .unique()
            .count();

        write!(f, "{}", "Summary:".bold())?;
        writeln!(
            f,
            " {} total node(s), totalling {} keys ({distinct_keys} distinct).",
            self.nodes.len(),
            total_keys
        )?;
        writeln!(
            f,
            "{}",
            "Note: Listed connections are tried from Left to Right".italic(),
        )?;

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct FlakePrefetch {
    pub(crate) hash: NixHash,
    #[serde(rename = "storePath")]
    pub(crate) store_path: SafeStorePath<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HiveLocation {
    HiveNix(PathBuf),
    Flake {
        uri: String,
        prefetch: FlakePrefetch,
    },
}

impl HiveLocation {
    async fn get_flake(uri: String, modifiers: SubCommandModifiers) -> Result<Self, HiveLibError> {
        let mut command_string = CommandStringBuilder::nix();
        command_string.args(&[
            "flake",
            "prefetch",
            "--extra-experimental-features",
            "nix-command",
            "--extra-experimental-features",
            "flakes",
            "--json",
        ]);
        command_string.arg(&uri);

        let command = run_command(
            &CommandArguments::new(command_string, modifiers)
                .mode(crate::commands::ChildOutputMode::Generic),
        )
        .await?;

        let result = command
            .wait_till_success()
            .await
            .map_err(HiveLibError::CommandError)?;

        debug!(hash_json = ?result);

        let prefetch = serde_json::from_str(&match result {
            Either::Left((.., output)) | Either::Right((.., output)) => output,
        })
        .map_err(|x| {
            HiveLibError::HiveInitialisationError(HiveInitialisationError::ParsePrefetchError(x))
        })?;

        debug!(prefetch = ?prefetch);

        Ok(Self::Flake { uri, prefetch })
    }
}

pub async fn get_hive_location(
    path: String,
    modifiers: SubCommandModifiers,
) -> Result<HiveLocation, HiveLibError> {
    let flakeref = FlakeRef::from_str(&path);

    let path_to_location = async |path: PathBuf| {
        Ok(match path.file_name().and_then(OsStr::to_str) {
            Some("hive.nix") => HiveLocation::HiveNix(path.clone()),
            Some(_) => {
                if fs::metadata(path.join("flake.nix")).is_ok() {
                    HiveLocation::get_flake(path.display().to_string(), modifiers).await?
                } else {
                    HiveLocation::HiveNix(path.join("hive.nix"))
                }
            }
            None => {
                return Err(HiveLibError::HiveLocationError(
                    HiveLocationError::MalformedPath(path.clone()),
                ));
            }
        })
    };

    match flakeref {
        Err(nix_compat::flakeref::FlakeRefError::UrlParseError(_err)) => {
            let path = PathBuf::from(path);
            Ok(path_to_location(path).await?)
        }
        Ok(FlakeRef::Path { path, .. }) => Ok(path_to_location(path).await?),
        Ok(
            FlakeRef::Git { .. }
            | FlakeRef::GitHub { .. }
            | FlakeRef::GitLab { .. }
            | FlakeRef::Tarball { .. }
            | FlakeRef::Mercurial { .. }
            | FlakeRef::SourceHut { .. },
        ) => Ok(HiveLocation::get_flake(path, modifiers).await?),
        Err(err) => Err(HiveLibError::HiveLocationError(
            HiveLocationError::Malformed(err),
        )),
        Ok(flakeref) => Err(HiveLibError::HiveLocationError(
            HiveLocationError::TypeUnsupported(Box::new(flakeref)),
        )),
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        errors::CommandError,
        get_test_path,
        hive::steps::keys::{Key, Source, UploadKeyAt},
        location,
        test_support::make_flake_sandbox,
    };

    use super::*;
    use std::assert_matches;
    use std::env;

    // flake should always come before hive.nix
    #[tokio::test]
    #[cfg_attr(feature = "no_eval_tests", ignore)]
    async fn test_hive_dot_nix_priority() {
        let location = location!(get_test_path!());

        assert_matches!(location, HiveLocation::Flake { .. });
    }

    #[tokio::test]
    #[cfg_attr(feature = "no_web_tests", ignore)]
    #[cfg_attr(feature = "no_eval_tests", ignore)]
    async fn test_hive_file() {
        let location = location!(get_test_path!());

        let hive = Hive::new_from_path(&location, None.into(), SubCommandModifiers::default())
            .await
            .unwrap();

        let node = Node {
            target: node::Target::from_host("192.168.122.96"),
            ..Default::default()
        };

        let mut nodes = HashMap::new();
        nodes.insert(Name("node-a".into()), node);

        assert_eq!(
            hive,
            Hive {
                nodes,
                schema: SchemaVersion::DeprecatedInteger(Hive::DEPRECATED_SCHEMA_VERSION)
            }
        );
    }

    #[tokio::test]
    #[cfg_attr(feature = "no_web_tests", ignore)]
    #[cfg_attr(feature = "no_eval_tests", ignore)]
    async fn non_trivial_hive() {
        let location = location!(get_test_path!());

        let hive = Hive::new_from_path(&location, None.into(), SubCommandModifiers::default())
            .await
            .unwrap();

        let node = Node {
            target: node::Target::from_host("name"),
            keys: vec![
                Key {
                    name: "different-than-a".into(),
                    dest_dir: "/run/keys/".into(),
                    path: "/run/keys/different-than-a".into(),
                    group: "root".into(),
                    user: "root".into(),
                    permissions: "0600".into(),
                    source: Source::String("hi".into()),
                    upload_at: UploadKeyAt::PreActivation,
                    environment: im::HashMap::new(),
                }
                .into(),
            ],
            build_remotely: true,
            ..Default::default()
        };

        let mut nodes = HashMap::new();
        nodes.insert(Name("node-a".into()), node);

        assert_eq!(
            hive,
            Hive {
                nodes,
                schema: SchemaVersion::DeprecatedInteger(Hive::DEPRECATED_SCHEMA_VERSION)
            }
        );
    }

    #[tokio::test]
    #[cfg_attr(feature = "no_web_tests", ignore)]
    #[cfg_attr(feature = "no_eval_tests", ignore)]
    async fn flake_hive() {
        let tmp_dir = make_flake_sandbox(&get_test_path!()).unwrap();

        let location = get_hive_location(
            tmp_dir.path().display().to_string(),
            SubCommandModifiers::default(),
        )
        .await
        .unwrap();
        let hive = Hive::new_from_path(&location, None.into(), SubCommandModifiers::default())
            .await
            .unwrap();

        let mut nodes = HashMap::new();

        // a merged node
        nodes.insert(Name("node-a".into()), Node::from_host("node-a"));
        // a non-merged node
        nodes.insert(Name("node-b".into()), Node::from_host("node-b"));

        assert_eq!(
            hive,
            Hive {
                nodes,
                schema: SchemaVersion::DeprecatedInteger(Hive::DEPRECATED_SCHEMA_VERSION)
            }
        );

        tmp_dir.close().unwrap();
    }

    #[tokio::test]
    #[cfg_attr(feature = "no_eval_tests", ignore)]
    async fn no_nixpkgs() {
        let location = location!(get_test_path!());

        assert_matches!(
            Hive::new_from_path(&location, None.into(), SubCommandModifiers::default()).await,
            Err(HiveLibError::NixEvalError {
                source: CommandError::CommandFailed {
                    logs,
                    ..
                },
                ..
            })
            if logs.contains("makeHive called without meta.nixpkgs specified")
        );
    }

    #[tokio::test]
    #[cfg_attr(feature = "no_eval_tests", ignore)]
    async fn _keys_should_fail() {
        let location = location!(get_test_path!());

        assert_matches!(
            Hive::new_from_path(&location, None.into(), SubCommandModifiers::default()).await,
            Err(HiveLibError::NixEvalError {
                source: CommandError::CommandFailed {
                    logs,
                    ..
                },
                ..
            })
            if logs.contains("The option `deployment._keys' is read-only, but it's set multiple times.")
        );
    }

    #[tokio::test]
    #[cfg_attr(feature = "no_eval_tests", ignore)]
    async fn test_force_always_local() {
        let mut location: PathBuf = env::var("WIRE_TEST_DIR").unwrap().into();
        location.push("non_trivial_hive");
        let location = location!(location);

        let mut hive = Hive::new_from_path(&location, None.into(), SubCommandModifiers::default())
            .await
            .unwrap();

        assert_matches!(
            hive.force_always_local(vec!["non-existent".to_string()]),
            Err(HiveLibError::HiveInitialisationError(
                HiveInitialisationError::NodeDoesNotExist(node)
            )) if node == "non-existent"
        );

        for node in hive.nodes.values() {
            assert!(node.build_remotely);
        }

        assert_matches!(hive.force_always_local(vec!["node-a".to_string()]), Ok(()));

        assert!(
            !hive
                .nodes
                .get(&Name("node-a".into()))
                .unwrap()
                .build_remotely
        );
    }
}
