use worktable::persistence::PersistenceEngine;
use worktable::prelude::*;
use worktable::worktable;

worktable!(
    name: FormatCompat,
    persist: true,
    columns: {
        id: String primary_key,
        indexed: u64,
        body: String,
    },
    indexes: {
        indexed_idx: indexed,
    },
);

#[tokio::main]
async fn main() {
    let root = std::env::args().nth(1).expect("store path");
    let config = DiskConfig::new_with_table_name(
        root,
        FormatCompatWorkTable::name_snake_case(),
        FormatCompatWorkTable::version(),
    );
    let engine = FormatCompatPersistenceEngine::new(config).await.unwrap();
    let table = FormatCompatWorkTable::load(engine).await.unwrap();

    assert_eq!(table.count(), 100);
    for number in 0..100 {
        let id = format!("row-{number}");
        let row = table.select(id.clone()).unwrap_or_else(|| panic!("missing {id}"));
        assert_eq!(row.indexed, number % 7);
        assert_eq!(row.body, format!("payload-{number}"));
    }

    let indexed_rows = table.select_by_indexed(3).execute().unwrap();
    assert_eq!(indexed_rows.len(), 14);
    table.close().await.unwrap();
}
