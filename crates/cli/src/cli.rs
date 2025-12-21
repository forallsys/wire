// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

use clap::builder::PossibleValue;
use clap::{Args, Parser, Subcommand, ValueEnum};
use clap::{ValueHint, crate_version};
use clap_complete::CompletionCandidate;
use clap_complete::engine::ArgValueCompleter;
use clap_num::number_range;
use clap_verbosity_flag::InfoLevel;
use tokio::runtime::Handle;
use wire_core::SubCommandModifiers;
use wire_core::commands::common::get_hive_node_names;
use wire_core::hive::node::{Goal as HiveGoal, HandleUnreachable, Name, SwitchToConfigurationGoal};
use wire_core::hive::{Hive, get_hive_location};

use std::io::IsTerminal;
use std::{
    fmt::{self, Display, Formatter},
    sync::Arc,
};

#[allow(clippy::struct_excessive_bools)]
#[derive(Parser)]
#[command(
    name = "wire",
    bin_name = "wire",
    about = "a tool to deploy nixos systems",
    version = format!("{}\nDebug: Hive::SCHEMA_VERSION {}", crate_version!(), Hive::SCHEMA_VERSION)
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    #[command(flatten)]
    pub verbose: clap_verbosity_flag::Verbosity<InfoLevel>,

    /// Path or flake reference
    #[arg(long, global = true, default_value = std::env::current_dir().unwrap().into_os_string(), visible_alias("flake"))]
    pub path: String,

    /// Hide progress bars.
    ///
    /// Defaults to true if stdin does not refer to a tty (unix pipelines, in CI).
    #[arg(long, global = true, default_value_t = !std::io::stdin().is_terminal())]
    pub no_progress: bool,

    /// Never accept user input.
    ///
    /// Defaults to true if stdin does not refer to a tty (unix pipelines, in CI).
    #[arg(long, global = true, default_value_t = !std::io::stdin().is_terminal())]
    pub non_interactive: bool,

    /// Show trace logs
    #[arg(long, global = true, default_value_t = false)]
    pub show_trace: bool,

    #[cfg(debug_assertions)]
    #[arg(long, hide = true, global = true)]
    pub markdown_help: bool,
}

#[derive(Clone, Debug)]
pub enum ApplyTarget {
    Node(Name),
    Tag(String),
    Stdin,
}

impl From<String> for ApplyTarget {
    fn from(value: String) -> Self {
        if value == "-" {
            return ApplyTarget::Stdin;
        }

        if let Some(stripped) = value.strip_prefix("@") {
            ApplyTarget::Tag(stripped.to_string())
        } else {
            ApplyTarget::Node(Name(Arc::from(value.as_str())))
        }
    }
}

impl Display for ApplyTarget {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            ApplyTarget::Node(name) => name.fmt(f),
            ApplyTarget::Tag(tag) => write!(f, "@{tag}"),
            ApplyTarget::Stdin => write!(f, "#stdin"),
        }
    }
}

fn more_than_zero(s: &str) -> Result<usize, String> {
    number_range(s, 1, usize::MAX)
}

fn parse_partitions(s: &str) -> Result<Partitions, String> {
    let parts: [&str; 2] = s
        .split('/')
        .collect::<Vec<_>>()
        .try_into()
        .map_err(|_| "parition must contain exactly one '/'")?;

    let (current, maximum) =
        std::array::from_fn(|i| parts[i].parse::<usize>().map_err(|x| x.to_string())).into();
    let (current, maximum) = (current?, maximum?);

    if current > maximum {
        return Err("current is more than total".to_string());
    }

    Ok(Partitions { current, maximum })
}

#[derive(Clone)]
pub enum HandleUnreachableArg {
    Ignore,
    FailNode,
}

impl Display for HandleUnreachableArg {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ignore => write!(f, "ignore"),
            Self::FailNode => write!(f, "fail-node"),
        }
    }
}

impl clap::ValueEnum for HandleUnreachableArg {
    fn value_variants<'a>() -> &'a [Self] {
        &[Self::Ignore, Self::FailNode]
    }

    fn to_possible_value(&self) -> Option<clap::builder::PossibleValue> {
        match self {
            Self::Ignore => Some(PossibleValue::new("ignore")),
            Self::FailNode => Some(PossibleValue::new("fail-node")),
        }
    }
}

impl From<HandleUnreachableArg> for HandleUnreachable {
    fn from(value: HandleUnreachableArg) -> Self {
        match value {
            HandleUnreachableArg::Ignore => Self::Ignore,
            HandleUnreachableArg::FailNode => Self::FailNode,
        }
    }
}

#[derive(Args)]
pub struct CommonVerbArgs {
    /// List of literal node names, a literal `-`, or `@` prefixed tags.
    ///
    /// `-` will read additional values from stdin, separated by whitespace.
    /// Any `-` implies `--non-interactive`.
    #[arg(short, long, value_name = "NODE | @TAG | `-`", num_args = 1.., add = ArgValueCompleter::new(node_names_completer), value_hint = ValueHint::Unknown)]
    pub on: Vec<ApplyTarget>,

    #[arg(short, long, default_value_t = 10, value_parser=more_than_zero)]
    pub parallel: usize,
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Args)]
pub struct ApplyArgs {
    #[command(flatten)]
    pub common: CommonVerbArgs,

    #[arg(value_enum, default_value_t)]
    pub goal: Goal,

    /// Skip key uploads. noop when [GOAL] = Keys
    #[arg(short, long, default_value_t = false)]
    pub no_keys: bool,

    /// Overrides deployment.buildOnTarget.
    #[arg(short, long, value_name = "NODE")]
    pub always_build_local: Vec<String>,

    /// Reboot the nodes after activation
    #[arg(short, long, default_value_t = false)]
    pub reboot: bool,

    /// Enable `--substitute-on-destination` in Nix subcommands.
    #[arg(short, long, default_value_t = true)]
    pub substitute_on_destination: bool,

    /// How to handle an unreachable node in the ping step.
    ///
    /// This only effects the ping step.
    /// wire will still fail the node if it becomes unreachable after activation
    #[arg(long, default_value_t = HandleUnreachableArg::FailNode)]
    pub handle_unreachable: HandleUnreachableArg,

    /// Unconditionally accept SSH host keys [!!]
    ///
    /// Sets `StrictHostKeyChecking` to `no`.
    /// Vulnerable to man-in-the-middle attacks, use with caution.
    #[arg(long, default_value_t = false)]
    pub ssh_accept_host: bool,
}

#[derive(Clone, Debug)]
pub struct Partitions {
    current: usize,
    maximum: usize,
}

#[derive(Args)]
pub struct BuildArgs {
    #[command(flatten)]
    pub common: CommonVerbArgs,

    /// Partition builds into buckets.
    ///
    /// In the format of `current_bucket/total_buckets`.
    #[arg(short = 'P', default_value="1/1", long, value_parser=parse_partitions)]
    pub partition: Option<Partitions>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Deploy nodes
    Apply(ApplyArgs),
    /// Build nodes offline
    ///
    /// This is distinct from `wire apply build`, as it will not ping or push
    /// the result, making it useful for CI.
    ///
    /// Additionally, you may parition the build jobs into buckets.
    Build(BuildArgs),
    /// Inspect hive
    #[clap(visible_alias = "show")]
    Inspect {
        #[arg(value_enum, default_value_t)]
        selection: Inspection,

        /// Return in JSON format
        #[arg(short, long, default_value_t = false)]
        json: bool,
    },
}

#[derive(Clone, Debug, Default, ValueEnum, Display)]
pub enum Inspection {
    /// Output all data wire has on the entire hive
    #[default]
    Full,
    /// Only output a list of node names
    Names,
}

#[derive(Clone, Debug, Default, ValueEnum, Display)]
pub enum Goal {
    /// Make the configuration the boot default and activate now
    #[default]
    Switch,
    /// Build the configuration & push the results
    Build,
    /// Copy the system derivation to the remote hosts
    Push,
    /// Push deployment keys to the remote hosts
    Keys,
    /// Activate the system profile on next boot
    Boot,
    /// Activate the configuration, but don't make it the boot default
    Test,
    /// Show what would be done if this configuration were activated.
    DryActivate,
}

impl TryFrom<Goal> for HiveGoal {
    type Error = miette::Error;

    fn try_from(value: Goal) -> Result<Self, Self::Error> {
        match value {
            Goal::Build => Ok(HiveGoal::Build),
            Goal::Push => Ok(HiveGoal::Push),
            Goal::Boot => Ok(HiveGoal::SwitchToConfiguration(
                SwitchToConfigurationGoal::Boot,
            )),
            Goal::Switch => Ok(HiveGoal::SwitchToConfiguration(
                SwitchToConfigurationGoal::Switch,
            )),
            Goal::Test => Ok(HiveGoal::SwitchToConfiguration(
                SwitchToConfigurationGoal::Test,
            )),
            Goal::DryActivate => Ok(HiveGoal::SwitchToConfiguration(
                SwitchToConfigurationGoal::DryActivate,
            )),
            Goal::Keys => Ok(HiveGoal::Keys),
        }
    }
}

pub trait ToSubCommandModifiers {
    fn to_subcommand_modifiers(&self) -> SubCommandModifiers;
}

impl ToSubCommandModifiers for Cli {
    fn to_subcommand_modifiers(&self) -> SubCommandModifiers {
        SubCommandModifiers {
            show_trace: self.show_trace,
            non_interactive: self.non_interactive,
            ssh_accept_host: match &self.command {
                Commands::Apply(args) if args.ssh_accept_host => {
                    wire_core::StrictHostKeyChecking::No
                }
                _ => wire_core::StrictHostKeyChecking::default(),
            },
        }
    }
}

fn node_names_completer(current: &std::ffi::OsStr) -> Vec<CompletionCandidate> {
    tokio::task::block_in_place(|| {
        let handle = Handle::current();
        let modifiers = SubCommandModifiers::default();
        let mut completions = vec![];

        if current.is_empty() || current == "-" {
            completions.push(
                CompletionCandidate::new("-").help(Some("Read stdin as --on arguments".into())),
            );
        }

        let Ok(current_dir) = std::env::current_dir() else {
            return completions;
        };

        let Ok(hive_location) = handle.block_on(get_hive_location(
            current_dir.display().to_string(),
            modifiers,
        )) else {
            return completions;
        };

        let Some(current) = current.to_str() else {
            return completions;
        };

        if current.starts_with('@') {
            return vec![];
        }

        if let Ok(names) =
            handle.block_on(async { get_hive_node_names(&hive_location, modifiers).await })
        {
            for name in names {
                if name.starts_with(current) {
                    completions.push(CompletionCandidate::new(name));
                }
            }
        }

        completions
    })
}

#[cfg(test)]
mod tests {
    use std::assert_matches::assert_matches;

    use crate::cli::{Partitions, parse_partitions};

    #[test]
    fn test_partitions() {
        assert_matches!(parse_partitions(""), Err(..));
        assert_matches!(parse_partitions("/"), Err(..));
        assert_matches!(parse_partitions(" / "), Err(..));
        assert_matches!(parse_partitions("abc/"), Err(..));
        assert_matches!(parse_partitions("abc"), Err(..));
        assert_matches!(parse_partitions("1/1"), Ok(Partitions {
            current,
            maximum
        }) if current == 1 && maximum == 1);
        assert_matches!(parse_partitions("0/1"), Ok(Partitions { current, maximum }) if current == 0 && maximum == 1);
        assert_matches!(parse_partitions("-11/1"), Err(..));
        assert_matches!(parse_partitions("100/99"), Err(..));
        assert_matches!(parse_partitions("5/10"), Ok(Partitions { current, maximum }) if current == 5 && maximum == 10);
    }
}
