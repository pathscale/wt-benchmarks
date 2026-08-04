use redb::{Database, MultimapTableDefinition, ReadableDatabase, TableDefinition};
use wt_footprint_campaign::{encoded_checksum, encoded_row};

macro_rules! define_table {
    ($module:ident, $table:literal, $index:literal) => {
        mod $module {
            use super::*;

            const ROWS: TableDefinition<u64, &[u8]> = TableDefinition::new($table);
            const ACCOUNT_INDEX: MultimapTableDefinition<u64, u64> =
                MultimapTableDefinition::new($index);

            pub fn touch(database: &Database, seed: u64) -> Result<u64, redb::Error> {
                let write = database.begin_write()?;
                {
                    let mut rows = write.open_table(ROWS)?;
                    let mut index = write.open_multimap_table(ACCOUNT_INDEX)?;
                    let encoded = encoded_row(seed, 14);
                    rows.insert(seed, encoded.as_slice())?;
                    index.insert(seed % 10_000, seed)?;
                }
                write.commit()?;

                let read = database.begin_read()?;
                let rows = read.open_table(ROWS)?;
                let index = read.open_multimap_table(ACCOUNT_INDEX)?;
                let row = rows.get(seed)?.expect("inserted row");
                let indexed = index.get(seed % 10_000)?.count() as u64;
                Ok(encoded_checksum(row.value()).expect("valid encoded row") ^ indexed)
            }
        }
    };
}

define_table!(table01, "footprint01", "footprint01_account_idx");
#[cfg(feature = "tables-2")]
define_table!(table02, "footprint02", "footprint02_account_idx");
#[cfg(feature = "tables-4")]
define_table!(table03, "footprint03", "footprint03_account_idx");
#[cfg(feature = "tables-4")]
define_table!(table04, "footprint04", "footprint04_account_idx");
#[cfg(feature = "tables-8")]
define_table!(table05, "footprint05", "footprint05_account_idx");
#[cfg(feature = "tables-8")]
define_table!(table06, "footprint06", "footprint06_account_idx");
#[cfg(feature = "tables-8")]
define_table!(table07, "footprint07", "footprint07_account_idx");
#[cfg(feature = "tables-8")]
define_table!(table08, "footprint08", "footprint08_account_idx");

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!(
        "wt-footprint-redb-{}-{}.redb",
        std::process::id(),
        std::thread::current().name().unwrap_or("main")
    ));
    let mut database = Database::create(&path)?;
    let mut checksum = table01::touch(&database, 1)?;
    #[cfg(feature = "tables-2")]
    {
        checksum ^= table02::touch(&database, 2)?;
    }
    #[cfg(feature = "tables-4")]
    {
        checksum ^= table03::touch(&database, 3)? ^ table04::touch(&database, 4)?;
    }
    #[cfg(feature = "tables-8")]
    {
        checksum ^= table05::touch(&database, 5)?
            ^ table06::touch(&database, 6)?
            ^ table07::touch(&database, 7)?
            ^ table08::touch(&database, 8)?;
    }
    while database.compact()? {}
    drop(database);
    std::fs::remove_file(path)?;
    println!("{checksum}");
    Ok(())
}
