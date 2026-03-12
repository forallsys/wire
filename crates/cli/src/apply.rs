// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

use futures::{FutureExt, StreamExt};
use itertools::{Either, Itertools};
use miette::{Diagnostic, IntoDiagnostic, Result};
use std::any::Any;
use std::collections::HashSet;
use std::io::{Read, stderr};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use thiserror::Error;
use tracing::{error, info};
use wire_core::hive::executor::execute;
use wire_core::hive::node::{Name, Node};
use wire_core::hive::plan::{Goal, plan_for_node};
use wire_core::hive::{Hive, HiveLocation};
use wire_core::status::STATUS;
use wire_core::{SubCommandModifiers, errors::HiveLibError};

use crate::cli::{ApplyTarget, CommonVerbArgs, Partitions};

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

fn resolve_targets(on: &[ApplyTarget]) -> Result<(HashSet<String>, HashSet<Name>)> {
    on.iter()
        .try_fold((HashSet::new(), HashSet::new()), |result, target| {
            let (mut tags, mut names) = result;
            match target {
                ApplyTarget::Tag(tag) => {
                    tags.insert(tag.clone());
                }
                ApplyTarget::Node(name) => {
                    names.insert(name.clone());
                }
                ApplyTarget::Stdin => {
                    let (found_tags, found_names) = read_apply_targets_from_stdin()?;

                    names.extend(found_names);
                    tags.extend(found_tags);
                }
            }
            Ok((tags, names))
        })
}

fn partition_arr<T>(arr: Vec<T>, partition: &Partitions) -> Vec<T>
where
    T: Any + Clone,
{
    if arr.is_empty() {
        return arr;
    }

    let items_per_chunk = arr.len().div_ceil(partition.maximum);

    arr.chunks(items_per_chunk)
        .nth(partition.current - 1)
        .unwrap_or(&[])
        .to_vec()
}

pub async fn apply<F>(
    hive: &mut Hive,
    should_quit: Arc<AtomicBool>,
    location: HiveLocation,
    args: CommonVerbArgs,
    partition: Partitions,
    make_goal: F,
    modifiers: SubCommandModifiers,
) -> Result<()>
where
    F: Fn(&Name, &Node) -> Goal,
{
    let location = Arc::new(location);

    // stdin implies non_interactive
    let mut modifiers = modifiers;
    if args.on.iter().any(|t| matches!(t, ApplyTarget::Stdin)) {
        modifiers.non_interactive = true;
    }

    let (tags, names) = resolve_targets(&args.on)?;

    let selected_names: Vec<_> = hive
        .nodes
        .iter()
        .filter(|(name, node)| {
            args.on.is_empty()
                || names.contains(name)
                || node.tags.iter().any(|tag| tags.contains(tag))
        })
        .sorted_by_key(|(name, _)| *name)
        .map(|(name, _)| name.clone())
        .collect();

    let partitioned_names = partition_arr(selected_names, &partition);

    STATUS
        .lock()
        .add_many(&partitioned_names.iter().collect::<Vec<_>>());

    if args.dry_run {
        dry_activate_nodes(
            hive,
            &partitioned_names,
            make_goal,
            &location,
            &should_quit,
            modifiers,
        );

        return Ok(());
    }

    apply_nodes(
        hive,
        &partitioned_names,
        make_goal,
        location,
        should_quit,
        args.parallel,
        modifiers,
    )
    .await
}

fn dry_activate_nodes<F>(
    hive: &Hive,
    names: &[Name],
    make_goal: F,
    location: &Arc<HiveLocation>,
    should_quit: &Arc<AtomicBool>,
    modifiers: SubCommandModifiers,
) where
    F: Fn(&Name, &Node) -> Goal,
{
    for name in names {
        let node = hive.nodes.get(name).unwrap();

        let goal = make_goal(name, node);
        let plan = plan_for_node(
            node,
            name.clone(),
            &goal,
            location.clone(),
            &modifiers,
            should_quit.clone(),
        );

        let goal_str = match &goal {
            Goal::Build => "Build".to_string(),
            Goal::Apply(args) => format!("Apply {:?}", args.goal),
        };

        println!("Node: {name}");
        println!("Goal: {goal_str}");
        println!("Steps:");
        for step in &plan.steps {
            println!("  - {step}");
        }
        println!();
    }
}

async fn apply_nodes<F>(
    hive: &mut Hive,
    names: &[Name],
    make_goal: F,
    location: Arc<HiveLocation>,
    should_quit: Arc<AtomicBool>,
    parallel: usize,
    modifiers: SubCommandModifiers,
) -> Result<()>
where
    F: Fn(&Name, &Node) -> Goal,
{
    let mut set = hive
        .nodes
        .iter_mut()
        .filter(|(name, _)| names.contains(name))
        .map(|(name, node)| {
            let goal = make_goal(name, node);
            let plan = plan_for_node(
                node,
                name.clone(),
                &goal,
                location.clone(),
                &modifiers,
                should_quit.clone(),
            );
            execute(plan).map(move |result| (name, result))
        })
        .peekable();

    if set.peek().is_none() {
        error!("There are no nodes selected for deployment");
    }

    let futures = futures::stream::iter(set).buffer_unordered(parallel);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::too_many_lines)]
    fn test_partitioning() {
        let arr = (1..=10).collect::<Vec<_>>();
        assert_eq!(arr, partition_arr(arr.clone(), &Partitions::default()));

        assert_eq!(
            vec![1, 2, 3, 4, 5],
            partition_arr(
                arr.clone(),
                &Partitions {
                    current: 1,
                    maximum: 2
                }
            )
        );
        assert_eq!(
            vec![6, 7, 8, 9, 10],
            partition_arr(
                arr,
                &Partitions {
                    current: 2,
                    maximum: 2
                }
            )
        );

        // test odd number
        let arr = (1..10).collect::<Vec<_>>();
        assert_eq!(
            arr.clone(),
            partition_arr(arr.clone(), &Partitions::default())
        );

        assert_eq!(
            vec![1, 2, 3, 4, 5],
            partition_arr(
                arr.clone(),
                &Partitions {
                    current: 1,
                    maximum: 2
                }
            )
        );
        assert_eq!(
            vec![6, 7, 8, 9],
            partition_arr(
                arr.clone(),
                &Partitions {
                    current: 2,
                    maximum: 2
                }
            )
        );

        // test large number of partitions
        let arr = (1..=10).collect::<Vec<_>>();
        assert_eq!(
            arr.clone(),
            partition_arr(arr.clone(), &Partitions::default())
        );

        for i in 1..=10 {
            assert_eq!(
                vec![i],
                partition_arr(
                    arr.clone(),
                    &Partitions {
                        current: i,
                        maximum: 10
                    }
                )
            );

            assert_eq!(
                vec![i],
                partition_arr(
                    arr.clone(),
                    &Partitions {
                        current: i,
                        maximum: 15
                    }
                )
            );
        }

        // stretching thin with higher partitions will start to leave higher ones empty
        assert_eq!(
            Vec::<usize>::new(),
            partition_arr(
                arr,
                &Partitions {
                    current: 11,
                    maximum: 15
                }
            )
        );

        // test the above holds for a lot of numbers
        for i in 1..1000 {
            let arr: Vec<usize> = (0..i).collect();
            let total = arr.len();

            assert_eq!(
                arr.clone(),
                partition_arr(arr.clone(), &Partitions::default()),
            );

            let buckets = 2;
            let chunk_size = total.div_ceil(buckets);
            let split_index = std::cmp::min(chunk_size, total);

            assert_eq!(
                &arr.clone()[..split_index],
                partition_arr(
                    arr.clone(),
                    &Partitions {
                        current: 1,
                        maximum: 2
                    }
                ),
            );
            assert_eq!(
                &arr.clone()[split_index..],
                partition_arr(
                    arr.clone(),
                    &Partitions {
                        current: 2,
                        maximum: 2
                    }
                ),
            );
        }
    }
}
