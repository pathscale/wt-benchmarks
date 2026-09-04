//! Bulk construction and retirement for one dense MoE-PGO map.
//!
//! [`crate::moe_pgo`] measures counter accumulation and version publication
//! under live readers. This companion isolates the generated bulk mutation
//! APIs used to construct or clear one map: `insert_many`, `delete_many`, and
//! `delete_range`, each against its loop of single-row calls.

use std::time::Duration;

use crate::moe_pgo::Backend;

macro_rules! moe_bulk_backend {
    ($module:ident, $name:ident, $row:ident, $table:ident, $primary:ident, $using:ident) => {
        mod $module {
            use std::time::{Duration, Instant};

            use worktable::prelude::*;
            use worktable::worktable;

            worktable! {
                name: $name,
                persist: false,
                columns: {
                    neuron: u32 primary_key using $using,
                    expert: u16,
                    fires: u64,
                }
            }

            fn rows(width: u32) -> Vec<$row> {
                (0..width)
                    .map(|neuron| $row {
                        neuron,
                        expert: (neuron % 64) as u16,
                        fires: 0,
                    })
                    .collect()
            }

            async fn populated(width: u32) -> $table {
                let table = $table::default();
                let inserted = table
                    .insert_many(rows(width))
                    .await
                    .expect("insert_many fixture");
                assert_eq!(inserted.len(), width as usize);
                table
            }

            pub async fn insert_loop(width: u32) -> Duration {
                let table = $table::default();
                let rows = rows(width);
                let start = Instant::now();
                for row in rows {
                    table.insert(row).await.expect("unique neuron");
                }
                start.elapsed()
            }

            pub async fn insert_many(width: u32) -> Duration {
                let table = $table::default();
                let rows = rows(width);
                let start = Instant::now();
                let inserted = table.insert_many(rows).await.expect("insert_many");
                let elapsed = start.elapsed();
                assert_eq!(inserted.len(), width as usize);
                elapsed
            }

            pub async fn delete_loop(width: u32) -> Duration {
                let table = populated(width).await;
                let start = Instant::now();
                for neuron in 0..width {
                    table.delete(neuron).await.expect("delete");
                }
                start.elapsed()
            }

            pub async fn delete_many(width: u32) -> Duration {
                let table = populated(width).await;
                let keys: Vec<_> = (0..width).collect();
                let start = Instant::now();
                let deleted = table.delete_many(keys).await.expect("delete_many");
                let elapsed = start.elapsed();
                assert_eq!(deleted.len(), width as usize);
                elapsed
            }

            pub async fn delete_range(width: u32) -> Duration {
                let table = populated(width).await;
                let start = Instant::now();
                let deleted = table
                    .delete_range($primary::from(0u32)..$primary::from(width))
                    .await
                    .expect("delete_range");
                let elapsed = start.elapsed();
                assert_eq!(deleted.len(), width as usize);
                elapsed
            }
        }
    };
}

moe_bulk_backend!(
    wti,
    MoePgo2Wti,
    MoePgo2WtiRow,
    MoePgo2WtiWorkTable,
    MoePgo2WtiPrimaryKey,
    worktables_index
);
moe_bulk_backend!(
    congee,
    MoePgo2Congee,
    MoePgo2CongeeRow,
    MoePgo2CongeeWorkTable,
    MoePgo2CongeePrimaryKey,
    congee
);
moe_bulk_backend!(
    arctic,
    MoePgo2Arctic,
    MoePgo2ArcticRow,
    MoePgo2ArcticWorkTable,
    MoePgo2ArcticPrimaryKey,
    arctic
);

/// Insert every row through one generated call per row.
pub async fn insert_loop(backend: Backend, width: u32) -> Duration {
    match backend {
        Backend::WorktablesIndex => wti::insert_loop(width).await,
        Backend::Congee => congee::insert_loop(width).await,
        Backend::Arctic => arctic::insert_loop(width).await,
    }
}

/// Insert the complete map through one `insert_many` call.
pub async fn insert_many(backend: Backend, width: u32) -> Duration {
    match backend {
        Backend::WorktablesIndex => wti::insert_many(width).await,
        Backend::Congee => congee::insert_many(width).await,
        Backend::Arctic => arctic::insert_many(width).await,
    }
}

/// Delete every row through one generated call per row.
pub async fn delete_loop(backend: Backend, width: u32) -> Duration {
    match backend {
        Backend::WorktablesIndex => wti::delete_loop(width).await,
        Backend::Congee => congee::delete_loop(width).await,
        Backend::Arctic => arctic::delete_loop(width).await,
    }
}

/// Delete the complete map through one `delete_many` key batch.
pub async fn delete_many(backend: Backend, width: u32) -> Duration {
    match backend {
        Backend::WorktablesIndex => wti::delete_many(width).await,
        Backend::Congee => congee::delete_many(width).await,
        Backend::Arctic => arctic::delete_many(width).await,
    }
}

/// Delete the complete dense map through one primary-key range.
pub async fn delete_range(backend: Backend, width: u32) -> Duration {
    match backend {
        Backend::WorktablesIndex => wti::delete_range(width).await,
        Backend::Congee => congee::delete_range(width).await,
        Backend::Arctic => arctic::delete_range(width).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn every_backend_completes_every_bulk_shape() {
        for backend in Backend::ALL {
            assert!(insert_loop(backend, 32).await > Duration::ZERO);
            assert!(insert_many(backend, 32).await > Duration::ZERO);
            assert!(delete_loop(backend, 32).await > Duration::ZERO);
            assert!(delete_many(backend, 32).await > Duration::ZERO);
            assert!(delete_range(backend, 32).await > Duration::ZERO);
        }
    }
}
