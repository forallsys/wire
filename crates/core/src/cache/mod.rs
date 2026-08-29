// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

use std::{
    collections::HashMap,
    env,
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
};

use itertools::Itertools;
use sqlx::{
    Pool, Sqlite,
    migrate::{MigrateError, Migrator},
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};
use tokio::fs::create_dir_all;
use tracing::{Level, debug, error, info, instrument, trace};
use wire_nix_client::NixClient;

use crate::{
    SafeStorePath,
    commands::trace_nix_log_message,
    hive::{FlakePrefetch, Hive, SCHEMA_VERSION_STRING, node::Name},
};

#[derive(Clone)]
pub struct InspectionCache {
    pool: Pool<Sqlite>,
}

static MIGRATOR: Migrator = sqlx::migrate!("src/cache/migrations");

async fn get_cache_directory() -> Option<PathBuf> {
    let home = PathBuf::from(
        env::var("HOME")
            .inspect_err(|_| error!("HOME env var not found"))
            .ok()?,
    );

    trace!(home = ?home);

    let cache_home = env::var("XDG_CACHE_HOME")
        .inspect_err(|_| debug!("XDG_CACHE_HOME not found"))
        .ok()
        .map_or_else(|| home.join(".cache"), PathBuf::from);

    let cache_directory = cache_home.join("wire");

    trace!(cache_directory = ?cache_directory);

    let _ = create_dir_all(&cache_directory).await;

    Some(cache_directory)
}

impl InspectionCache {
    pub async fn new() -> Option<Self> {
        let cache_path = get_cache_directory().await?.join("inspect.db");
        debug!(cache_path = ?cache_path);

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(&cache_path)
                    .journal_mode(SqliteJournalMode::Wal)
                    .synchronous(SqliteSynchronous::Normal)
                    .create_if_missing(true),
            )
            .await
            .inspect_err(|x| error!("failed to open cache db: {x}"))
            .ok()?;

        let result = MIGRATOR.run(&pool).await;

        if let Err(ref err) = result {
            let recovered_pool = match err {
                MigrateError::VersionMissing(_) | MigrateError::VersionTooOld(_, _) => {
                    Some(Self::reset_migration(pool, &cache_path).await?)
                }
                _ => {
                    error!("failed to run cache migrations: {:?}", result);
                    return None;
                }
            };
            if let Some(pool) = recovered_pool {
                return Some(Self { pool });
            }
            error!("failed to run cache migrations: {:?}", result);
            return None;
        }

        Some(Self { pool })
    }

    async fn reset_migration(pool: Pool<Sqlite>, cache_path: &Path) -> Option<Pool<Sqlite>> {
        info!(
            "failed to run cache migrations, resetting {}",
            cache_path.to_string_lossy()
        );

        pool.close().await;

        tokio::fs::remove_file(cache_path)
            .await
            .inspect_err(|e| error!("failed to remove stale cache db: {e}"))
            .ok();

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(cache_path)
                    .create_if_missing(true),
            )
            .await
            .inspect_err(|x| error!("failed to create cache db after failed migrations: {x}"))
            .ok()?;

        MIGRATOR
            .run(&pool)
            .await
            .inspect_err(|err| error!("failed to run cache migrations after recreation: {err:?}"))
            .ok()?;

        Some(pool)
    }

    fn is_cache_invalid(store_path_name: &str, store_path_digest: &[u8]) -> bool {
        let store_path =
            match SafeStorePath::<&str>::from_name_and_digest(store_path_name, store_path_digest) {
                Err(err) => {
                    error!(err = %err, "failed to parse store digest & name from cache entry");

                    return true;
                }
                Ok(path) => path,
            };

        let absolute_path = store_path.to_absolute_path();
        let path = Path::new(&absolute_path);

        // possible TOCTOU
        !path.exists()
    }

    pub async fn get_hive(&self, prefetch: &FlakePrefetch) -> Option<Hive> {
        struct Query {
            json_value: Vec<u8>,
            store_path_digest: Vec<u8>,
            store_path_name: String,
        }

        let store_path_digest = &prefetch.store_path.digest()[..];
        let store_path_name = prefetch.store_path.name();
        let hash = prefetch.hash.to_sri_string();

        let cached_blob = sqlx::query_as!(
            Query,
            "
            select
              inspection_blobs.json_value,
              inspection_cache.store_path_digest,
              inspection_cache.store_path_name
            from
              inspection_blobs
              join inspection_cache on inspection_cache.blob_id = inspection_blobs.id
            where
              inspection_cache.store_path_digest = $1
              and inspection_cache.hash_sri = $2
              and inspection_blobs.schema_version = $3
              and inspection_cache.store_path_name = $4
            limit
              1
            ",
            store_path_digest,
            hash,
            &**SCHEMA_VERSION_STRING,
            store_path_name
        )
        .fetch_optional(&self.pool)
        .await
        .inspect_err(|x| error!("failed to fetch cached hive: {x}"))
        .ok()??;

        // the cached path may of been garbage collected, discard it
        // it is quite hard to replicate this bug but its occurred to me
        // atleast once
        if Self::is_cache_invalid(&cached_blob.store_path_name, &cached_blob.store_path_digest) {
            trace!("discarding cache that does not exist in the nix store");
            return None;
        }

        trace!(
            "read {} bytes of zstd data from cache",
            cached_blob.json_value.len()
        );

        let json_string = zstd::decode_all(cached_blob.json_value.as_slice())
            .inspect_err(|err| error!("failed to decode cached zstd data: {err}"))
            .ok()?;

        trace!(
            "inflated {} > {} in decoding",
            cached_blob.json_value.len(),
            json_string.len()
        );

        serde_json::from_slice(&json_string)
            .inspect_err(|err| {
                error!("could not use cached evaluation: {err}");
            })
            .ok()
    }

    pub async fn store_hive(&self, prefetch: &FlakePrefetch, json_value: &String) {
        let Ok(json_value) = zstd::encode_all(json_value.as_bytes(), 0)
            .inspect_err(|err| error!("failed to encode data w/ zstd: {err}"))
        else {
            return;
        };

        let hive_inspection = sqlx::query_scalar!(
            "
            insert into inspection_blobs (json_value, schema_version)
            values ($1, $2)
            on conflict(json_value)
            do update set json_value = excluded.json_value
            returning inspection_blobs.id
            ",
            json_value,
            &**SCHEMA_VERSION_STRING
        )
        .fetch_one(&self.pool)
        .await
        .inspect_err(|x| error!("could not insert hive_inspection: {x}"));

        let Ok(blob_id) = hive_inspection else {
            return;
        };

        let store_path_digest = &prefetch.store_path.digest()[..];
        let store_path_name = &prefetch.store_path.name();
        let hash_sri = prefetch.hash.to_sri_string();

        let cached_inspection = sqlx::query!(
            "
            insert into
              inspection_cache (store_path_digest, store_path_name, hash_sri, blob_id)
            values
              ($1, $2, $3, $4)
            ",
            store_path_digest,
            store_path_name,
            hash_sri,
            blob_id
        )
        .execute(&self.pool)
        .await;

        if let Err(err) = cached_inspection {
            error!("could not insert cached_inspection: {err}");
        }
    }

    #[instrument(skip(self), ret(level = Level::DEBUG))]
    pub async fn get_evaluations(
        &self,
        prefetch: &FlakePrefetch,
        nodes: &[Name],
        should_quit: Arc<AtomicBool>,
    ) -> Option<HashMap<Name, SafeStorePath<String>>> {
        struct Query {
            output_path_digest: Vec<u8>,
            output_path_name: String,
            node_name: String,
        }

        let mut client = NixClient::open_local(|log, map, print| trace_nix_log_message(log, map, print, None), should_quit, false).await.inspect_err(|err| error!(err = ?err, "failed to open local nix client to verify cached evaluations")).ok()?;

        let store_path_digest = &prefetch.store_path.digest()[..];
        let store_path_name = prefetch.store_path.name();
        let flake_hash_sri = prefetch.hash.to_sri_string();
        let nodes_json = serde_json::to_string(nodes).unwrap_or_else(|_| "[]".to_string());

        let evaluation_cache = sqlx::query_as!(
            Query,
            "
            select
                evaluation_cache.output_path_digest,
                evaluation_cache.output_path_name,
                evaluation_cache.node_name
            from
                evaluation_cache
            where
                evaluation_cache.flake_path_digest = $1
                and evaluation_cache.flake_path_name = $2
                and evaluation_cache.flake_hash_sri = $3
                and evaluation_cache.node_name in (select value from json_each($4))
        ",
            store_path_digest,
            store_path_name,
            flake_hash_sri,
            nodes_json
        )
        .fetch_all(&self.pool)
        .await
        .inspect_err(|x| error!("failed to fetch cached node evaluations: {x}"))
        .ok()?;

        // hashmap that maps the &str versions of the node `Name`s to their
        // `Name` reference.
        let node_names = nodes
            .iter()
            .map(|name| (name.0.as_ref(), name.clone()))
            .collect::<HashMap<_, _>>();

        let evaluation_cache: HashMap<_, _> = evaluation_cache
            .into_iter()
            .map(|x| {
                (
                    x.node_name,
                    SafeStorePath::<String>::from_name_and_digest(
                        &x.output_path_name,
                        &x.output_path_digest,
                    )
                    .inspect_err(|err| error!("failed to parse StorePath from cache: {err:?}"))
                    .ok(),
                )
            })
            .filter_map(|(name, path)| path.map(|path| (name, path)))
            .filter_map(|(name, path)| {
                node_names
                    .get(name.as_str())
                    .map(|name| (name.clone(), path))
            })
            .collect();

        let valid_paths = client
            .query_valid_paths(evaluation_cache.values().cloned().collect_vec(), false)
            .await
            .inspect_err(
                |err| error!(err = ?err, "failed to query valid paths for evaluation cache"),
            )
            .ok()?;

        // delete paths that no longer exist in the nix store
        let invalid_paths: Vec<_> = evaluation_cache
            .iter()
            .filter(|(_, path)| !valid_paths.contains(path))
            .map(|(_, path)| (path.digest().to_vec(), path.name().clone()))
            .collect();

        if !invalid_paths.is_empty() {
            let mut query_builder = sqlx::QueryBuilder::new(
                "delete from evaluation_cache where (output_path_digest, output_path_name) in ",
            );
            query_builder.push_tuples(invalid_paths.into_iter(), |mut b, (digest, name)| {
                b.push_bind(digest).push_bind(name);
            });

            if let Err(err) = query_builder.build().execute(&self.pool).await {
                error!(err = ?err, "failed to delete invalid evaluation cache entries");
            }
        }

        Some(
            evaluation_cache
                .into_iter()
                .filter(|(_, path)| valid_paths.contains(path))
                .collect(),
        )
    }

    pub async fn store_evaluation(
        &self,
        prefetch: &FlakePrefetch,
        name: &Name,
        path: SafeStorePath<String>,
    ) {
        trace!(evaluated_path = ?path, prefetch = ?prefetch, "storing evaluated output");

        let store_path_digest = &prefetch.store_path.digest()[..];
        let store_path_name = prefetch.store_path.name();
        let output_path_digest = &path.digest()[..];
        let output_path_name = path.name();
        let flake_hash_sri = prefetch.hash.to_sri_string();
        let node_name = name.to_string();

        let cached_evaluation = sqlx::query!(
            "
            insert into evaluation_cache (
                    flake_path_digest,
                    flake_path_name,
                    flake_hash_sri,
                    node_name,
                    output_path_digest,
                    output_path_name
            ) values ($1, $2, $3, $4, $5, $6)
        ",
            store_path_digest,
            store_path_name,
            flake_hash_sri,
            node_name,
            output_path_digest,
            output_path_name
        )
        .execute(&self.pool)
        .await;

        if let Err(err) = cached_evaluation {
            error!("could not insert cached_evaluation: {err}");
        }
    }

    pub async fn gc(&self, should_quit: Arc<AtomicBool>) -> Result<(), sqlx::Error> {
        // keep newest 30 AND
        // delete caches that refer to a blob w/ wrong schema
        sqlx::query!(
            "delete from inspection_cache
where
  blob_id in (
    select
      id
    from
      inspection_blobs
    where
      schema_version != $1
  )
  or ROWID in (
    select
      ROWID
    from
      inspection_cache
    order by
      ROWID desc
    limit
      -1
    offset
      30
  )",
            &**SCHEMA_VERSION_STRING
        )
        .execute(&self.pool)
        .await?;

        // delete orphaned blobs
        sqlx::query!(
            "delete from inspection_blobs
where
  not exists (
    select
      1
    from
      inspection_cache
    where
      inspection_cache.blob_id = inspection_blobs.id
  )"
        )
        .execute(&self.pool)
        .await?;

        self.gc_evaluation_cache(should_quit).await?;

        Ok(())
    }

    pub async fn gc_evaluation_cache(
        &self,
        should_quit: Arc<AtomicBool>,
    ) -> Result<(), sqlx::Error> {
        struct EvaluationPaths {
            flake_path_digest: Vec<u8>,
            flake_path_name: String,
            output_path_digest: Vec<u8>,
            output_path_name: String,
            rowid: i64,
        }

        let mut previous_rowid = 0;

        let mut client = match NixClient::open_local(
            |log, map, print| trace_nix_log_message(log, map, print, None),
            should_quit,
            false,
        )
        .await
        {
            Ok(c) => c,
            Err(err) => {
                error!(err = ?err, "failed to open local nix client for gc");
                return Ok(());
            }
        };

        // delete caches whose flake/output paths no longer exist in the nix store
        loop {
            let evaluation_paths: Vec<_> = sqlx::query_as!(
                EvaluationPaths,
                "select rowid, flake_path_digest, flake_path_name, output_path_digest, output_path_name from evaluation_cache where rowid > $1 order by rowid asc limit 50",
                previous_rowid
            )
            .fetch_all(&self.pool)
            .await?;

            if evaluation_paths.is_empty() {
                break;
            }

            previous_rowid = evaluation_paths.last().map_or(previous_rowid, |r| r.rowid);

            // build list of all store paths to check
            let all_paths: Vec<SafeStorePath<String>> = gen {
                for path in &evaluation_paths {
                    if let Ok(p) = SafeStorePath::<String>::from_name_and_digest(
                        &path.flake_path_name,
                        &path.flake_path_digest,
                    ) {
                        yield p;
                    }
                    if let Ok(p) = SafeStorePath::<String>::from_name_and_digest(
                        &path.output_path_name,
                        &path.output_path_digest,
                    ) {
                        yield p;
                    }
                }
            }
            .collect();

            // query which cached paths are valid in the nix store
            let valid_paths = match client.query_valid_paths(all_paths.clone(), false).await {
                Ok(v) => v,
                Err(err) => {
                    error!(err = ?err, "failed to query valid paths for gc");
                    return Ok(());
                }
            };

            let valid_set: std::collections::HashSet<_> = valid_paths.into_iter().collect();

            // delete invalid evaluation_cache entries
            let invalid_entries: Vec<(&Vec<u8>, &String, &Vec<u8>, &String)> = evaluation_paths
                .iter()
                .filter(|path| {
                    // is invalid if either the output or the flake path are not in
                    // the valid set
                    if let Ok(p) = SafeStorePath::<String>::from_name_and_digest(
                        &path.flake_path_name,
                        &path.flake_path_digest,
                    ) && !valid_set.contains(&p)
                    {
                        true
                    } else if let Ok(p) = SafeStorePath::<String>::from_name_and_digest(
                        &path.output_path_name,
                        &path.output_path_digest,
                    ) && !valid_set.contains(&p)
                    {
                        true
                    } else {
                        false
                    }
                })
                .map(|path| {
                    (
                        &path.flake_path_digest,
                        &path.flake_path_name,
                        &path.output_path_digest,
                        &path.output_path_name,
                    )
                })
                .collect();

            if !invalid_entries.is_empty() {
                let mut query_builder = sqlx::QueryBuilder::new(
                    "delete from evaluation_cache where (flake_path_digest, flake_path_name, output_path_digest, output_path_name) in ",
                );
                query_builder.push_tuples(invalid_entries, |mut b, (fd, fn_, od, on_)| {
                    b.push_bind(fd).push_bind(fn_).push_bind(od).push_bind(on_);
                });

                if let Err(err) = query_builder.build().execute(&self.pool).await {
                    error!(err = ?err, "failed to delete from evaluation path");
                }
            }
        }

        Ok(())
    }
}
