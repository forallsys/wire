// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

use std::{env, path::PathBuf};

use sqlx::{
    Pool, Sqlite,
    migrate::Migrator,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use tokio::fs::create_dir_all;
use tracing::{debug, error, trace};

use crate::hive::{FlakePrefetch, Hive};

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

    pub async fn get_hive(&self, prefetch: &FlakePrefetch) -> Option<Hive> {
        let cached_blob: Vec<u8> = sqlx::query_scalar!(
            "
            select inspection_blobs.json_value from inspection_blobs
            join inspection_cache
            on inspection_cache.blob_id = inspection_blobs.id
            where inspection_cache.store_path = $1
            and inspection_cache.hash = $2
            and inspection_blobs.schema_version = $3
            limit 1
            ",
            prefetch.store_path,
            prefetch.hash,
            Hive::SCHEMA_VERSION
        )
        .fetch_optional(&self.pool)
        .await
        .inspect_err(|x| error!("failed to fetch cached hive: {x}"))
        .ok()??;

        trace!("read {} bytes of zstd data from cache", cached_blob.len());

        let json_string = zstd::decode_all(cached_blob.as_slice())
            .inspect_err(|err| error!("failed to decode cached zstd data: {err}"))
            .ok()?;

        trace!(
            "inflated {} > {} in decoding",
            cached_blob.len(),
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

        let cached_inspection = sqlx::query!(
            "
            insert into
              inspection_cache (store_path, hash, blob_id)
            values
              ($1, $2, $3)
            ",
            prefetch.store_path,
            prefetch.hash,
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
