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
    write_rows(&table);
    table.wait_for_ops().await;
}

fn write_rows(table: &FormatCompatWorkTable) {
    for number in 0..100 {
        table
            .insert(FormatCompatRow {
                id: format!("row-{number}"),
                indexed: number % 7,
                body: format!("payload-{number}"),
            })
            .unwrap();
    }
}
