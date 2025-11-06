// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

use itertools::Itertools;
use nix_compat::flakeref::FlakeRef;
use node::{Name, Node};
use owo_colors::{OwoColorize, Stream};
use serde::de::Error;
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashMap;
use std::collections::hash_map::OccupiedEntry;
use std::ffi::OsStr;
use std::fmt::Display;
use std::fs;
use std::path::PathBuf;
use std::str::FromStr;
use tracing::{info, instrument};

use crate::commands::common::evaluate_hive_attribute;
use crate::errors::{HiveInitializationError, HiveLocationError};
use crate::{EvalGoal, HiveLibError, SubCommandModifiers};
pub mod node;
pub mod steps;

#[derive(Serialize, Deserialize, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Hive {
    pub nodes: HashMap<Name, Node>,

    #[serde(deserialize_with = "check_schema_version", rename = "_schema")]
    pub schema: u32,
}

pub type ShallowHive = HashMap<Name, Vec<String>>;

pub enum Action<'a> {
    Inspect,
    EvaluateNode(OccupiedEntry<'a, String, Node>),
}

fn check_schema_version<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    let version = u32::deserialize(d)?;
    if version != Hive::SCHEMA_VERSION {
        return Err(D::Error::custom(
            "Version mismatch for Hive. Please ensure the binary and your wire input match!",
        ));
    }
    Ok(version)
}

impl Hive {
    pub const SCHEMA_VERSION: u32 = 2;

    #[instrument(skip_all, name = "eval_hive")]
    pub async fn new_from_path(
        location: &HiveLocation,
        modifiers: SubCommandModifiers,
    ) -> Result<Hive, HiveLibError> {
        info!("evaluating hive {location:?}");

        let output = evaluate_hive_attribute(location, &EvalGoal::Inspect, modifiers).await?;

        let hive: Hive = serde_json::from_str(&output).map_err(|err| {
            HiveLibError::HiveInitializationError(HiveInitializationError::ParseEvaluateError(err))
        })?;

        Ok(hive)
    }

    #[instrument(skip_all, name = "eval_names")]
    pub async fn shallow_from_path(
        location: &HiveLocation,
        modifiers: SubCommandModifiers,
    ) -> Result<ShallowHive, HiveLibError> {
        info!("evaluating hive {location:?} names");

        let output = evaluate_hive_attribute(location, &EvalGoal::Shallow, modifiers).await?;

        let names: ShallowHive = serde_json::from_str(&output).map_err(|err| {
            HiveLibError::HiveInitializationError(HiveInitializationError::ParseEvaluateError(err))
        })?;

        Ok(names)
    }

    #[instrument(skip_all, name = "eval_names")]
    pub async fn node_from_path<'a>(
        name: &'a Name,
        location: &'a HiveLocation,
        modifiers: SubCommandModifiers,
    ) -> Result<Node, HiveLibError> {
        info!("evaluating {name:?} from {location:?}");

        let output =
            evaluate_hive_attribute(location, &EvalGoal::InspectNode(name), modifiers).await?;

        let names: Node = serde_json::from_str(&output).map_err(|err| {
            HiveLibError::HiveInitializationError(HiveInitializationError::ParseEvaluateError(err))
        })?;

        Ok(names)
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
            .collect::<Vec<_>>();
        let distinct_keys = self
            .nodes
            .values()
            .flat_map(|node| node.keys.iter())
            .unique()
            .collect::<Vec<_>>()
            .len();

        write!(f, "{}", "Summary:".bold())?;
        writeln!(
            f,
            " {} total node(s), totalling {} keys ({distinct_keys} distinct).",
            self.nodes.len(),
            total_keys.len()
        )?;
        writeln!(
            f,
            "{}",
            "Note: Listed connections are tried from Left to Right".italic(),
        )?;

        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum HiveLocation {
    HiveNix(PathBuf),
    Flake(String),
}

pub fn get_hive_location(path: String) -> Result<HiveLocation, HiveLocationError> {
    let flakeref = FlakeRef::from_str(&path);

    let path_to_location = |path: PathBuf| {
        Ok(match path.file_name().and_then(OsStr::to_str) {
            Some("hive.nix") => HiveLocation::HiveNix(path.clone()),
            Some(_) => {
                if fs::metadata(path.join("flake.nix")).is_ok() {
                    HiveLocation::Flake(path.display().to_string())
                } else {
                    HiveLocation::HiveNix(path.join("hive.nix"))
                }
            }
            None => return Err(HiveLocationError::MalformedPath(path.clone())),
        })
    };

    match flakeref {
        Err(nix_compat::flakeref::FlakeRefError::UrlParseError(_err)) => {
            let path = PathBuf::from(path);
            Ok(path_to_location(path)?)
        }
        Ok(FlakeRef::Path { path, .. }) => Ok(path_to_location(path)?),
        Ok(
            FlakeRef::Git { .. }
            | FlakeRef::GitHub { .. }
            | FlakeRef::GitLab { .. }
            | FlakeRef::Tarball { .. }
            | FlakeRef::Mercurial { .. }
            | FlakeRef::SourceHut { .. },
        ) => Ok(HiveLocation::Flake(path)),
        Err(err) => Err(HiveLocationError::Malformed(err)),
        Ok(flakeref) => Err(HiveLocationError::TypeUnsupported(Box::new(flakeref))),
    }
}

#[cfg(test)]
mod tests {
    use im::vector;

    use crate::{
        errors::CommandError,
        get_test_path,
        hive::steps::keys::{Key, Source, UploadKeyAt},
        location,
        test_support::make_flake_sandbox,
    };

    use super::*;
    use std::{assert_matches::assert_matches, env};

    // flake should always come before hive.nix
    #[test]
    fn test_hive_dot_nix_priority() {
        let location = location!(get_test_path!());

        assert_matches!(location, HiveLocation::Flake(..));
    }

    #[tokio::test]
    #[cfg_attr(feature = "no_web_tests", ignore)]
    async fn test_hive_file() {
        let location = location!(get_test_path!());

        let hive = Hive::new_from_path(&location, SubCommandModifiers::default())
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
                schema: Hive::SCHEMA_VERSION
            }
        );
    }

    #[tokio::test]
    #[cfg_attr(feature = "no_web_tests", ignore)]
    async fn non_trivial_hive() {
        let location = location!(get_test_path!());

        let hive = Hive::new_from_path(&location, SubCommandModifiers::default())
            .await
            .unwrap();

        let node = Node {
            target: node::Target::from_host("name"),
            keys: vector![Key {
                name: "different-than-a".into(),
                dest_dir: "/run/keys/".into(),
                path: "/run/keys/different-than-a".into(),
                group: "root".into(),
                user: "root".into(),
                permissions: "0600".into(),
                source: Source::String("hi".into()),
                upload_at: UploadKeyAt::PreActivation,
                environment: im::HashMap::new()
            }],
            build_remotely: true,
            ..Default::default()
        };

        let mut nodes = HashMap::new();
        nodes.insert(Name("node-a".into()), node);

        assert_eq!(
            hive,
            Hive {
                nodes,
                schema: Hive::SCHEMA_VERSION
            }
        );
    }

    #[tokio::test]
    #[cfg_attr(feature = "no_web_tests", ignore)]
    async fn flake_hive() {
        let tmp_dir = make_flake_sandbox(&get_test_path!()).unwrap();

        let location = get_hive_location(tmp_dir.path().display().to_string()).unwrap();
        let hive = Hive::new_from_path(&location, SubCommandModifiers::default())
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
                schema: Hive::SCHEMA_VERSION
            }
        );

        tmp_dir.close().unwrap();
    }

    #[tokio::test]
    async fn no_nixpkgs() {
        let location = location!(get_test_path!());

        assert_matches!(
            Hive::new_from_path(&location, SubCommandModifiers::default()).await,
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
    async fn _keys_should_fail() {
        let location = location!(get_test_path!());

        assert_matches!(
            Hive::new_from_path(&location, SubCommandModifiers::default()).await,
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
}
