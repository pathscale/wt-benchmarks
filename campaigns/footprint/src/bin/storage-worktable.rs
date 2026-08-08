use std::time::Duration;

use tokio::time::timeout;
use worktable::prelude::*;
use worktable::worktable;
use wt_footprint_campaign::{StorageConfig, emit_storage, live_rows_after_churn, payload};

worktable!(
    name: StorageFootprint,
    persist: true,
    columns: {
        id: u64 primary_key,
        account_id: u64,
        sequence: u64,
        score: f64,
        payload: String,
    },
    indexes: {
        account_idx: account_id,
    },
    queries: {
        update: {
            SetPayload(payload) by id,
        }
    }
);

type BenchResult<T> = Result<T, Box<dyn std::error::Error>>;

#[tokio::main]
async fn main() -> BenchResult<()> {
    let config = StorageConfig::parse().unwrap_or_else(|error| {
        eprintln!("error: {error}");
        std::process::exit(2);
    });
    let disk_config = DiskConfig::new_with_table_name(
        config.path.to_string_lossy(),
        StorageFootprintWorkTable::name_snake_case(),
        StorageFootprintWorkTable::version(),
    );
    let engine = StorageFootprintPersistenceEngine::new(disk_config.clone()).await?;
    let table = StorageFootprintWorkTable::load(engine).await?;

    for id in 0..config.rows {
        table.insert(StorageFootprintRow {
            id,
            account_id: id % 10_000,
            sequence: id.wrapping_mul(17),
            score: id as f64 / 100.0,
            payload: payload(id, config.payload_bytes),
        })?;
        if (id + 1) % config.drain_every == 0 {
            drain(&table).await?;
        }
    }
    drain(&table).await?;
    if table.count() as u64 != config.rows {
        return Err("WorkTable row count mismatch after load".into());
    }
    emit_storage("worktable-persisted", "loaded", &config, config.rows)?;

    for id in 0..config.rows {
        match id % 4 {
            0 => {
                table.delete(id).await?;
            }
            1 => {
                table
                    .update_set_payload(
                        SetPayloadQuery {
                            payload: payload(id.wrapping_add(1_000_000), config.payload_bytes),
                        },
                        id,
                    )
                    .await?;
            }
            _ => {}
        }
        if (id + 1) % config.drain_every == 0 {
            drain(&table).await?;
        }
    }
    drain(&table).await?;
    let expected = live_rows_after_churn(config.rows);
    if table.count() as u64 != expected {
        return Err("WorkTable row count mismatch after churn".into());
    }
    emit_storage("worktable-persisted", "churned", &config, expected)?;

    let vacuum_stats = table.vacuum().vacuum().await?;
    drain(&table).await?;
    emit_storage("worktable-persisted", "vacuumed", &config, expected)?;
    eprintln!(
        "WorkTable vacuum: pages_processed={} pages_freed={} bytes_freed={}",
        vacuum_stats.pages_processed, vacuum_stats.pages_freed, vacuum_stats.bytes_freed
    );

    drop(table);
    let engine = StorageFootprintPersistenceEngine::new(disk_config).await?;
    let reloaded = StorageFootprintWorkTable::load(engine).await?;
    if reloaded.count() as u64 != expected {
        return Err("WorkTable row count mismatch after reload".into());
    }
    if reloaded.select(0).is_some() {
        return Err("WorkTable reload resurrected a deleted row".into());
    }
    if config.rows > 1 {
        let row = reloaded
            .select(1)
            .ok_or("WorkTable reload lost an expected row")?;
        if row.payload != payload(1_000_001, config.payload_bytes) {
            return Err("WorkTable reload returned the wrong updated payload".into());
        }
        let indexed = reloaded.select_by_account_id(1).execute()?;
        if !indexed.iter().any(|candidate| candidate.id == 1) {
            return Err("WorkTable reload lost a secondary-index entry".into());
        }
    }
    drain(&reloaded).await?;
    emit_storage("worktable-persisted", "reloaded", &config, expected)?;
    drop(reloaded);
    emit_storage(
        "worktable-persisted",
        "closed-after-reload",
        &config,
        expected,
    )?;
    Ok(())
}

async fn drain(table: &StorageFootprintWorkTable) -> BenchResult<()> {
    timeout(Duration::from_secs(120), table.wait_for_ops())
        .await
        .map_err(|_| "WorkTable persistence drain timed out")??;
    Ok(())
}
