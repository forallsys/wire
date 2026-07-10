// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

use futures::{FutureExt, StreamExt};
use itertools::{Either, Itertools};
use miette::{Diagnostic, IntoDiagnostic, Result};
use std::any::Any;
use std::collections::HashSet;
use std::io::Read;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use thiserror::Error;
use tokio::sync::oneshot;
use tracing::{error, info};
use wire_core::cache::InspectionCache;
use wire_core::hive::executor::execute;
use wire_core::hive::node::{Name, Node};
use wire_core::hive::plan::{Goal, plan_for_node};
use wire_core::hive::{Hive, HiveLocation};
use wire_core::status::{UI_SENDER, UiMessage};
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
    std::io::stdin()
        .lock()
        .read_to_string(&mut buf)
        .into_diagnostic()?;

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
) -> Result<(HashSet<String>, HashSet<Name>)> {
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
                    modifiers.non_interactive = true;
                    let (found_tags, found_names) = read_apply_targets_from_stdin()?;
                    names.extend(found_names);
                    tags.extend(found_tags);
                }
            }
            Ok((tags, names))
        })
}

const fn partition_slice<'a, T>(arr: &'a [T], partition: &Partitions) -> &'a [T]
where
    T: Any + Clone,
{
    if arr.is_empty() {
        return arr;
    }

    let items_per_chunk = arr.len().div_ceil(partition.maximum);
    let chunk_index = partition.current.saturating_sub(1);
    let start = chunk_index * items_per_chunk;

    if start >= arr.len() {
        return &[];
    }

    let end = konst::cmp::min!(start + items_per_chunk, arr.len());

    konst::slice::slice_range(arr, start, end)
}

#[allow(clippy::missing_errors_doc)]
pub async fn apply<F>(
    hive: &mut Hive,
    should_quit: Arc<AtomicBool>,
    location: HiveLocation,
    args: CommonVerbArgs,
    partition: Partitions,
    make_goal: F,
    mut modifiers: SubCommandModifiers,
    cache: Arc<Option<InspectionCache>>,
) -> Result<()>
where
    F: Fn(&Name, &Node) -> Goal + Send + Sync,
{
    let location = Arc::new(location);

    let (tags, names) = resolve_targets(&args.on, &mut modifiers)?;

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

    let mut cached_evaluations = if let Some(ref cache) = *cache.clone()
        && let HiveLocation::Flake { ref prefetch, .. } = *location
    {
        cache
            .get_evaluations(prefetch, &selected_names, should_quit.clone())
            .await
    } else {
        None
    };

    let num_selected = selected_names.len();

    let partitioned_names = partition_slice(&selected_names, &partition).to_vec();

    if num_selected != partitioned_names.len() {
        info!(
            "Partitioning reduced selected number of nodes from {num_selected} to {}",
            partitioned_names.len()
        );
    }

    if let Some(tx) = UI_SENDER.get() {
        let _ = tx.send(UiMessage::AddMany(partitioned_names.clone()));
    }

    let mut evaluation_cache_tasks = Vec::new();

    let mut set = hive
        .nodes
        .iter_mut()
        .filter(|(name, _)| partitioned_names.contains(name))
        .map(|(name, node)| {
            let goal = make_goal(name, node);

            let plan = plan_for_node(
                node,
                name.clone(),
                &goal,
                location.clone(),
                &modifiers,
                should_quit.clone(),
                cached_evaluations
                    .as_mut()
                    .and_then(|cache| cache.remove(name))
                    .as_ref(),
            );

            let (sender, receiver) = oneshot::channel();

            let location = location.clone();
            let cache = cache.clone();
            let cache_name = name.clone();

            evaluation_cache_tasks.push(tokio::spawn(async move {
                if let Some(ref cache) = *cache
                    && let HiveLocation::Flake { ref prefetch, .. } = *location
                    && let Ok(evaluated_path) = receiver.await
                {
                    cache
                        .store_evaluation(prefetch, &cache_name, evaluated_path)
                        .await;
                }
            }));

            let name_clone = name.clone();
            execute(plan, sender).map(move |result| (name_clone, result))
        })
        .peekable();

    if set.peek().is_none() {
        error!("There are no nodes selected for deployment");
    }

    let futures = futures::stream::iter(set).buffer_unordered(args.parallel);
    let result = futures.collect::<Vec<_>>().await;

    for task in evaluation_cache_tasks {
        let _ = task.await;
    }

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
            successful.into_iter().map(|x| x.0).collect_vec()
        );
    }

    // clear the status bar at the end of execution.
    if let Some(tx) = UI_SENDER.get() {
        let _ = tx.send(UiMessage::Clear);
    }

    if !errors.is_empty() {
        return Err(NodeErrors(
            errors
                .into_iter()
                .map(|(name, error)| NodeError(name, error))
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
        assert_eq!(arr, partition_slice(&arr.clone(), &Partitions::default()));

        assert_eq!(
            vec![1, 2, 3, 4, 5],
            partition_slice(
                &arr,
                &Partitions {
                    current: 1,
                    maximum: 2
                }
            )
        );
        assert_eq!(
            vec![6, 7, 8, 9, 10],
            partition_slice(
                &arr,
                &Partitions {
                    current: 2,
                    maximum: 2
                }
            )
        );

        // test odd number
        let arr = (1..10).collect::<Vec<_>>();
        assert_eq!(arr, partition_slice(&arr.clone(), &Partitions::default()));

        assert_eq!(
            vec![1, 2, 3, 4, 5],
            partition_slice(
                &arr,
                &Partitions {
                    current: 1,
                    maximum: 2
                }
            )
        );
        assert_eq!(
            vec![6, 7, 8, 9],
            partition_slice(
                &arr,
                &Partitions {
                    current: 2,
                    maximum: 2
                }
            )
        );

        // test large number of partitions
        let arr = (1..=10).collect::<Vec<_>>();
        assert_eq!(arr, partition_slice(&arr.clone(), &Partitions::default()));

        for i in 1..=10 {
            assert_eq!(
                vec![i],
                partition_slice(
                    &arr,
                    &Partitions {
                        current: i,
                        maximum: 10
                    }
                )
            );

            assert_eq!(
                vec![i],
                partition_slice(
                    &arr.clone(),
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
            partition_slice(
                &arr,
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
                partition_slice(&arr.clone(), &Partitions::default()),
            );

            let buckets = 2;
            let chunk_size = total.div_ceil(buckets);
            let split_index = std::cmp::min(chunk_size, total);

            assert_eq!(
                &arr.clone()[..split_index],
                partition_slice(
                    &arr.clone(),
                    &Partitions {
                        current: 1,
                        maximum: 2
                    }
                ),
            );
            assert_eq!(
                &arr.clone()[split_index..],
                partition_slice(
                    &arr.clone(),
                    &Partitions {
                        current: 2,
                        maximum: 2
                    }
                ),
            );
        }
    }
}
