use std::collections::HashSet;

use itertools::Itertools;
use tracing::info;

use crate::{
    SafeStorePath,
    errors::HiveLibError,
    hive::node::{Context, Push, SharedTarget},
    nix_client::{NixClient, WireAddToStoreNarRequest},
};

pub async fn push(
    context: &Context,
    target: &SharedTarget,
    push: Push<'_>,
    substitute_on_destination: bool,
) -> Result<(), HiveLibError> {
    let mut local_daemon = NixClient::open_local().await?;

    let target = target.0.read().await;

    let (mut remote_daemon, host) = NixClient::open_remote(&target, context.modifiers).await?;

    let path = match push {
        Push::Derivation(path) | Push::Path(path) => path.clone(),
    };

    info!(path = ?path, "attempting to push");

    let closure = local_daemon.collect_complete_closure(&path).await?;

    info!(path = ?path, "closure has {:?} paths", closure.len());

    let paths_on_target = remote_daemon
        .query_valid_paths(
            closure.clone().into_iter().collect_vec(),
            substitute_on_destination,
        )
        .await?
        .into_iter()
        .collect::<HashSet<_>>();

    info!(path = ?path, "target already has {} path(s)", paths_on_target.len());

    let paths_to_upload = closure.into_iter().filter(|p| !paths_on_target.contains(p));

    for path in paths_to_upload {
        info!("copying '{}' to node {host}", path.to_absolute_path());

        let Some(path_info) = local_daemon.query(&path).await? else {
            return Err(HiveLibError::NixDaemonOperationFailed(format!(
                "selected {path:?} for upload does not exist in local store"
            )));
        };

        let nar_stream = local_daemon
            .get_nar_stream(&path, path_info.nar_size)
            .await?;

        remote_daemon
            .add_to_store_nar(
                WireAddToStoreNarRequest {
                    path: path.clone(),
                    deriver: path_info.deriver.map(Into::into),
                    nar_hash: path_info.nar_hash,
                    references: path_info
                        .references
                        .into_iter()
                        .map(SafeStorePath)
                        .collect(),
                    registration_time: path_info.registration_time,
                    nar_size: path_info.nar_size,
                    ultimate: false,
                    signatures: path_info.signatures,
                    ca: path_info.ca,
                    repair: false,
                    dont_check_sigs: true,
                },
                nar_stream,
            )
            .await?;
    }

    Ok(())
}
