// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

use futures::{FutureExt, StreamExt};
use itertools::{Either, Itertools};
use miette::{Diagnostic, IntoDiagnostic, Result};
use std::collections::HashSet;
use std::io::{Read, stderr};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use thiserror::Error;
use tracing::{error, info};
use wire_core::hive::node::{Context, GoalExecutor, Name, Node, Objective, StepState};
use wire_core::hive::{Hive, HiveLocation};
use wire_core::status::STATUS;
use wire_core::{SubCommandModifiers, errors::HiveLibError};

use crate::cli::{ApplyTarget, CommonVerbArgs};

#[derive(Debug, Error, Diagnostic)]
#[error("node {} failed to apply", .0)]
struct NodeError(
    Name,
    #[source]
    #[diagnostic_source]
    HiveLibError,
);

#[derive(Debug, Error, Diagnostic)]
#[error("{} node(s) failed to apply.", .0.len())]
struct NodeErrors(#[related] Vec<NodeError>);

// returns Names and Tags
fn read_apply_targets_from_stdin() -> Result<(Vec<String>, Vec<Name>)> {
    let mut buf = String::new();
    let mut stdin = std::io::stdin().lock();
    stdin.read_to_string(&mut buf).into_diagnostic()?;

    Ok(buf
        .split_whitespace()
        .map(|x| ApplyTarget::from(x.to_string()))
        .fold((Vec::new(), Vec::new()), |(mut tags, mut names), target| {
            match target {
                ApplyTarget::Node(name) => names.push(name),
                ApplyTarget::Tag(tag) => tags.push(tag),
                ApplyTarget::Stdin => {}
            }
            (tags, names)
        }))
}

fn resolve_targets(
    on: &[ApplyTarget],
    modifiers: &mut SubCommandModifiers,
) -> (HashSet<String>, HashSet<Name>) {
    on.iter().fold(
        (HashSet::new(), HashSet::new()),
        |(mut tags, mut names), target| {
            match target {
                ApplyTarget::Tag(tag) => {
                    tags.insert(tag.clone());
                }
                ApplyTarget::Node(name) => {
                    names.insert(name.clone());
                }
                ApplyTarget::Stdin => {
                    // implies non_interactive
                    modifiers.non_interactive = true;

                    let (found_tags, found_names) = read_apply_targets_from_stdin().unwrap();
                    names.extend(found_names);
                    tags.extend(found_tags);
                }
            }
            (tags, names)
        },
    )
}

pub async fn apply<F>(
    hive: &mut Hive,
    should_shutdown: Arc<AtomicBool>,
    location: HiveLocation,
    args: CommonVerbArgs,
    make_objective: F,
    mut modifiers: SubCommandModifiers,
) -> Result<()>
where
    F: Fn(&Name, &mut Node) -> Objective,
{
    let location = Arc::new(location);

    let (tags, names) = resolve_targets(&args.on, &mut modifiers);

    let selected_nodes: Vec<_> = hive
        .nodes
        .iter_mut()
        .filter(|(name, node)| {
            args.on.is_empty()
                || names.contains(name)
                || node.tags.iter().any(|tag| tags.contains(tag))
        })
        .collect();

    STATUS.lock().add_many(
        &selected_nodes
            .iter()
            .map(|(name, _)| *name)
            .collect::<Vec<_>>(),
    );

    let mut set = selected_nodes
        .into_iter()
        .map(|(name, node)| {
            info!("Resolved {:?} to include {}", args.on, name);

            let objective = make_objective(name, node);

            let context = Context {
                node,
                name,
                objective,
                state: StepState::default(),
                hive_location: location.clone(),
                modifiers,
                should_quit: should_shutdown.clone(),
            };

            GoalExecutor::new(context)
                .execute()
                .map(move |result| (name, result))
        })
        .peekable();

    if set.peek().is_none() {
        error!("There are no nodes selected for deployment");
    }

    let futures = futures::stream::iter(set).buffer_unordered(args.parallel);
    let result = futures.collect::<Vec<_>>().await;
    let (successful, errors): (Vec<_>, Vec<_>) =
        result
            .into_iter()
            .partition_map(|(name, result)| match result {
                Ok(..) => Either::Left(name),
                Err(err) => Either::Right((name, err)),
            });

    if !successful.is_empty() {
        info!(
            "Successfully applied goal to {} node(s): {:?}",
            successful.len(),
            successful
        );
    }

    if !errors.is_empty() {
        // clear the status bar if we are about to print error messages
        STATUS.lock().clear(&mut stderr());

        return Err(NodeErrors(
            errors
                .into_iter()
                .map(|(name, error)| NodeError(name.clone(), error))
                .collect(),
        )
        .into());
    }

    Ok(())
}
