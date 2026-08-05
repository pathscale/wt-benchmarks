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
    let config = PersistenceConfig::new(format!("{root}/config"), root);
    let table = FormatCompatWorkTable::load_from_file(config).await.unwrap();
    for number in 0..100 {
        table
            .insert(FormatCompatRow {
                id: format!("row-{number}"),
                indexed: number % 7,
                body: format!("payload-{number}"),
            })
            .unwrap();
    }
    table.wait_for_ops().await;
}
