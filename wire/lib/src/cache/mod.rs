// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

use std::{env, fs, path::PathBuf, str::FromStr};

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
        let cached_json: String = sqlx::query_scalar!(
            "
            select hive_inspection.json_value from cached_inspection
            join hive_inspection 
            on cached_inspection.inspection_id = hive_inspection.id
            where cached_inspection.store_path = $1
            and cached_inspection.hash = $2
            limit 1
            ",
            prefetch.store_path,
            prefetch.hash
        )
        .fetch_optional(&self.pool)
        .await
        .inspect_err(|x| error!("failed to fetch cached hive: {x}"))
        .ok()??;

        serde_json::from_str(&cached_json)
            .inspect_err(|err| {
                error!("could not use cached evaluation: {err}");
            })
            .ok()
    }

    pub async fn store_hive(&self, prefetch: &FlakePrefetch, json_value: &String) {
        let hive_inspection = sqlx::query_scalar!(
            "
            insert into hive_inspection (json_value)
            values ($1)
            on conflict(json_value)
            do update set json_value = excluded.json_value
            returning hive_inspection.id
            ",
            json_value
        )
        .fetch_one(&self.pool)
        .await
        .inspect_err(|x| error!("could not insert hive_inspection: {x}"));

        let Ok(hive_inspection_id) = hive_inspection else {
            return;
        };

        let cached_inspection = sqlx::query!(
            "
            insert into cached_inspection (store_path, hash, inspection_id)
            values ($1, $2, $3)
            ",
            prefetch.store_path,
            prefetch.hash,
            hive_inspection_id
        )
        .execute(&self.pool)
        .await;

        if let Err(err) = cached_inspection {
            error!("could not insert cached_inspection: {err}");
        }
    }

    pub async fn gc(&self) -> Result<(), sqlx::Error> {
        let query = sqlx::query_as!(
            FlakePrefetch,
            "select store_path, hash from cached_inspection"
        )
        .fetch_all(&self.pool)
        .await?;

        let prefetch_to_drop = query.iter().filter(|prefetch| {
            !fs::exists(prefetch.store_path.clone())
                .inspect_err(|e| error!("cannot check existence of {}: {e}", prefetch.store_path))
                // true (negated, so not dropped) if the cached entry cant be read for some reason.
                .unwrap_or(true)
        });

        // there is certainly a faster and nicer way to do this in bulk,
        // but sqlite seems more limited.
        for prefetch in prefetch_to_drop {
            debug!("deleting cached entry for {prefetch:?}");
            sqlx::query!(
                "
                delete from cached_inspection
                where cached_inspection.store_path = $1 and cached_inspection.hash = $2
                ",
                prefetch.store_path,
                prefetch.hash
            )
            .execute(&self.pool)
            .await?;
        }

        // delete orphaned
        sqlx::query!(
            "delete from hive_inspection 
            where not exists (
                select 1 from cached_inspection
                where cached_inspection.inspection_id = hive_inspection.id
            )"
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}
