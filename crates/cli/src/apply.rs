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
use wire_core::hive::node::{Context, GoalExecutor, Name, Node, Objective, StepState};
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
    should_shutdown: Arc<AtomicBool>,
    location: HiveLocation,
    args: CommonVerbArgs,
    partition: Partitions,
    make_objective: F,
    mut modifiers: SubCommandModifiers,
) -> Result<()>
where
    F: Fn(&Name, &Node) -> Objective,
{
    let location = Arc::new(location);

    let (tags, names) = resolve_targets(&args.on, &mut modifiers);

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

    let num_selected = selected_names.len();

    let partitioned_names = partition_arr(selected_names, &partition);

    if num_selected != partitioned_names.len() {
        info!(
            "Partitioning reduced selected number of nodes from {num_selected} to {}",
            partitioned_names.len()
        );
    }

    STATUS
        .lock()
        .add_many(&partitioned_names.iter().collect::<Vec<_>>());

    let mut set = hive
        .nodes
        .iter_mut()
        .filter(|(name, _)| partitioned_names.contains(name))
        .map(|(name, node)| {
            info!("Resolved {:?} to include {}", args.on, name);

            let objective = make_objective(name, node);

            todo!()

            // let context = Context {
            //     node,
            //     name,
            //     objective,
            //     state: StepState::default(),
            //     hive_location: location.clone(),
            //     modifiers,
            //     should_quit: should_shutdown.clone(),
            // };
            //
            // GoalExecutor::new(context)
            //     .execute()
            //     .map(move |result| (name, result))
        })
        .peekable();

    if set.peek().is_none() {
        error!("There are no nodes selected for deployment");
    }

    // let futures = futures::stream::iter(set).buffer_unordered(args.parallel);
    // let result = futures.collect::<Vec<_>>().await;
    // let (successful, errors): (Vec<_>, Vec<_>) =
    //     result
    //         .into_iter()
    //         .partition_map(|(name, result)| match result {
    //             Ok(..) => Either::Left(name),
    //             Err(err) => Either::Right((name, err)),
    //         });
    //
    // if !successful.is_empty() {
    //     info!(
    //         "Successfully applied goal to {} node(s): {:?}",
    //         successful.len(),
    //         successful
    //     );
    // }
    //
    // if !errors.is_empty() {
    //     // clear the status bar if we are about to print error messages
    //     STATUS.lock().clear(&mut stderr());
    //
    //     return Err(NodeErrors(
    //         errors
    //             .into_iter()
    //             .map(|(name, error)| NodeError(name.clone(), error))
    //             .collect(),
    //     )
    //     .into());
    // }

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
