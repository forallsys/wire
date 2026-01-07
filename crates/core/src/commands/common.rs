// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

use std::collections::HashMap;

use tracing::instrument;

use crate::{
    EvalGoal, SubCommandModifiers,
    commands::{
        CommandArguments, Either, WireCommandChip, builder::CommandStringBuilder, run_command,
        run_command_with_env,
    },
    errors::{CommandError, HiveInitialisationError, HiveLibError},
    hive::{
        HiveLocation,
        node::{Context, Objective, Push},
    },
};

fn get_common_copy_path_help(error: &CommandError) -> Option<String> {
    if let CommandError::CommandFailed { logs, .. } = error
        && (logs.contains("error: unexpected end-of-file"))
    {
        Some("wire requires the deploying user or wire binary cache is trusted on the remote server. if you're attempting to make that change, skip keys with --no-keys. please read https://wire.forall.systems/guides/keys for more information".to_string())
    } else {
        None
    }
}

pub async fn push(context: &Context<'_>, push: Push<'_>) -> Result<(), HiveLibError> {
    let mut command_string = CommandStringBuilder::nix();

    command_string.args(&["--extra-experimental-features", "nix-command", "copy"]);
    if let Objective::Apply(apply_objective) = context.objective {
        command_string.opt_arg(
            apply_objective.substitute_on_destination,
            "--substitute-on-destination",
        );
    }
    command_string.arg("--to");
    command_string.args(&[
        format!(
            "ssh://{user}@{host}",
            user = context.node.target.user,
            host = context.node.target.get_preferred_host()?,
        ),
        match push {
            Push::Derivation(drv) => format!("{drv} --derivation"),
            Push::Path(path) => path.clone(),
        },
    ]);

    let child = run_command_with_env(
        &CommandArguments::new(command_string, context.modifiers)
            .mode(crate::commands::ChildOutputMode::Nix),
        HashMap::from([(
            "NIX_SSHOPTS".into(),
            context
                .node
                .target
                .create_ssh_opts(context.modifiers, false)?,
        )]),
    )
    .await?;

    let status = child.wait_till_success().await;

    let help = if let Err(ref error) = status {
        get_common_copy_path_help(error).map(Box::new)
    } else {
        None
    };

    status.map_err(|error| HiveLibError::NixCopyError {
        name: context.name.clone(),
        path: push.to_string(),
        error: Box::new(error),
        help,
    })?;

    Ok(())
}

fn get_common_command_help(error: &CommandError) -> Option<String> {
    if let CommandError::CommandFailed { logs, .. } = error
        // marshmallow: your using this repo as a hive you idiot
        && (logs.contains("attribute 'inspect' missing")
            // using a flake that does not provide `wire`
            || logs.contains("does not provide attribute 'packages.x86_64-linux.wire'")
            // using a file called `hive.nix` that is not actually a hive
            || logs.contains("attribute 'inspect' in selection path"))
    {
        Some("Double check this `--path` or `--flake` is a wire hive. You may be pointing to the wrong directory.".to_string())
    } else {
        None
    }
}

pub async fn get_hive_node_names(
    location: &HiveLocation,
    modifiers: SubCommandModifiers,
) -> Result<Vec<String>, HiveLibError> {
    let output = evaluate_hive_attribute(location, &EvalGoal::Names, modifiers).await?;
    serde_json::from_str(&output).map_err(|err| {
        HiveLibError::HiveInitialisationError(HiveInitialisationError::ParseEvaluateError(err))
    })
}

/// Evaluates the hive in flakeref with regards to the given goal,
/// and returns stdout.
#[instrument(ret(level = tracing::Level::TRACE), skip_all)]
pub async fn evaluate_hive_attribute(
    location: &HiveLocation,
    goal: &EvalGoal<'_>,
    modifiers: SubCommandModifiers,
) -> Result<String, HiveLibError> {
    let attribute = match location {
        HiveLocation::Flake { uri, .. } => {
            format!(
                "{uri}#wire --apply \"hive: {}\"",
                match goal {
                    EvalGoal::Inspect => "hive.inspect".to_string(),
                    EvalGoal::Names => "hive.names".to_string(),
                    EvalGoal::GetTopLevel(node) => format!("hive.topLevels.{node}"),
                }
            )
        }
        HiveLocation::HiveNix(path) => {
            format!(
                "--file {} {}",
                &path.to_string_lossy(),
                match goal {
                    EvalGoal::Inspect => "inspect".to_string(),
                    EvalGoal::Names => "names".to_string(),
                    EvalGoal::GetTopLevel(node) => format!("topLevels.{node}"),
                }
            )
        }
    };

    let mut command_string = CommandStringBuilder::nix();
    command_string.args(&[
        "--extra-experimental-features",
        "nix-command",
        "--extra-experimental-features",
        "flakes",
        "eval",
        "--json",
    ]);
    command_string.opt_arg(modifiers.show_trace, "--show-trace");
    command_string.arg(&attribute);

    let child = run_command(
        &CommandArguments::new(command_string, modifiers)
            .mode(crate::commands::ChildOutputMode::Nix),
    )
    .await?;

    let status = child.wait_till_success().await;

    let help = if let Err(ref error) = status {
        get_common_command_help(error).map(Box::new)
    } else {
        None
    };

    status
        .map_err(|source| HiveLibError::NixEvalError {
            attribute,
            source,
            help,
        })
        .map(|x| match x {
            Either::Left((_, stdout)) | Either::Right((_, stdout)) => stdout,
        })
}
