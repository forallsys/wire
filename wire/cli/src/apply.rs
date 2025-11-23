// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

use futures::{FutureExt, StreamExt};
use itertools::{Either, Itertools};
use lib::hive::node::{Context, GoalExecutor, Name, StepState, should_apply_locally};
use lib::hive::{Hive, HiveLocation};
use lib::status::STATUS;
use lib::{SubCommandModifiers, errors::HiveLibError};
use miette::{Diagnostic, IntoDiagnostic, Result};
use std::collections::HashSet;
use std::io::{Read, stderr};
use std::sync::Arc;
use thiserror::Error;
use tracing::{Span, error, info};

use crate::cli::{ApplyArgs, ApplyTarget};

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

// #[instrument(skip_all, fields(goal = %args.goal, on = %args.on.iter().join(", ")))]
pub async fn apply(
    hive: &mut Hive,
    location: HiveLocation,
    args: ApplyArgs,
    mut modifiers: SubCommandModifiers,
) -> Result<()> {
    let header_span = Span::current();
    let location = Arc::new(location);

    // Respect user's --always-build-local arg
    hive.force_always_local(args.always_build_local)?;

    let header_span_enter = header_span.enter();

    let (tags, names) = args.on.iter().fold(
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
    );

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

            let should_apply_locally = should_apply_locally(node.allow_local_deployment, &name.0);

            let context = Context {
                node,
                name,
                goal: args.goal.clone().try_into().unwrap(),
                state: StepState::default(),
                no_keys: args.no_keys,
                hive_location: location.clone(),
                modifiers,
                reboot: args.reboot,
                should_apply_locally,
                handle_unreachable: args.handle_unreachable.clone().into(),
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

    std::mem::drop(header_span_enter);
    std::mem::drop(header_span);

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
