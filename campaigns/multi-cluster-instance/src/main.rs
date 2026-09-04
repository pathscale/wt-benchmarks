//! Published beta.17: one generated table schema, two independently loaded
//! persisted instances, graceful close/reload, and independent unload.

use worktable::persistence::PersistenceEngine;
use worktable::prelude::*;
use worktable::worktable;

worktable! {
    name: ClusterObject,
    persist: true,
    columns: {
        key: u64 primary_key using arctic,
        cluster: u64,
        value: u64,
    }
}

fn config(root: &std::path::Path) -> DiskConfig {
    DiskConfig::new_with_table_name(
        root.to_string_lossy().into_owned(),
        ClusterObjectWorkTable::name_snake_case(),
        ClusterObjectWorkTable::version(),
    )
}

async fn open(root: &std::path::Path) -> ClusterObjectWorkTable {
    let engine = ClusterObjectPersistenceEngine::new(config(root))
        .await
        .expect("create persistence engine");
    ClusterObjectWorkTable::load(engine)
        .await
        .expect("load table instance")
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    let scratch = tempfile::tempdir().expect("scratch directory");
    let alpha_root = scratch.path().join("alpha");
    let beta_root = scratch.path().join("beta");

    let alpha = open(&alpha_root).await;
    let beta = open(&beta_root).await;
    alpha
        .insert(ClusterObjectRow {
            key: 11,
            cluster: 1,
            value: 101,
        })
        .await
        .expect("insert alpha");
    beta.insert(ClusterObjectRow {
        key: 22,
        cluster: 2,
        value: 202,
    })
    .await
    .expect("insert beta");
    alpha.wait_for_ops().await.expect("drain alpha");
    beta.wait_for_ops().await.expect("drain beta");

    assert_eq!(alpha.select(11).expect("alpha row").value, 101);
    assert!(alpha.select(22).is_none());
    assert_eq!(beta.select(22).expect("beta row").value, 202);
    assert!(beta.select(11).is_none());

    alpha.close().await.expect("close alpha");
    beta.close().await.expect("close beta");

    let alpha = open(&alpha_root).await;
    let beta = open(&beta_root).await;
    assert_eq!(alpha.select(11).expect("reloaded alpha").value, 101);
    assert_eq!(beta.select(22).expect("reloaded beta").value, 202);

    alpha.close().await.expect("unload alpha");
    assert_eq!(
        beta.select(22).expect("beta survives alpha unload").value,
        202
    );
    beta.close().await.expect("unload beta");

    println!("published_worktable=1.0.0-beta.17");
    println!("schema_instances=2");
    println!("isolated_queries=true");
    println!("graceful_close_reload=true");
    println!("independent_unload=true");
    println!("physical_layout=one_worktable_store_per_instance");
}

