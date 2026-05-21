// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

use std::{
    env,
    path::{Path, PathBuf},
};

use sqlx::{
    Pool, Sqlite,
    migrate::Migrator,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use tokio::fs::create_dir_all;
use tracing::{debug, error, trace};

use crate::{
    SafeStorePath,
    hive::{FlakePrefetch, Hive},
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
        .map(PathBuf::from)
        .unwrap_or(home.join(".cache"));

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
                    .filename(cache_path)
                    .create_if_missing(true),
            )
            .await
            .inspect_err(|x| error!("failed to open cache db: {x}"))
            .ok()?;

        MIGRATOR
            .run(&pool)
            .await
            .inspect_err(|err| error!("failed to run cache migrations: {err:?}"))
            .ok()?;

        Some(Self { pool })
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
            Hive::SCHEMA_VERSION,
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
            Hive::SCHEMA_VERSION
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

    pub async fn gc(&self) -> Result<(), sqlx::Error> {
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
            Hive::SCHEMA_VERSION
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

        Ok(())
    }
}
