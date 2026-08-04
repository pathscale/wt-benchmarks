use rusqlite::Connection;

macro_rules! define_table {
    ($module:ident, $table:literal, $index:literal) => {
        mod $module {
            use super::Connection;
            use rusqlite::params;

            pub fn touch(connection: &Connection, seed: u64) -> rusqlite::Result<u64> {
                connection.execute_batch(concat!(
                    "CREATE TABLE ",
                    $table,
                    " (id INTEGER PRIMARY KEY, account_id INTEGER NOT NULL, ",
                    "sequence INTEGER NOT NULL, score REAL NOT NULL, payload TEXT NOT NULL);",
                    "CREATE INDEX ",
                    $index,
                    " ON ",
                    $table,
                    "(account_id);"
                ))?;
                connection.execute(
                    concat!("INSERT INTO ", $table, " VALUES (?1, ?2, ?3, ?4, ?5)"),
                    params![
                        seed as i64,
                        (seed % 10_000) as i64,
                        seed.wrapping_mul(17) as i64,
                        seed as f64 / 100.0,
                        "payloadpayload"
                    ],
                )?;
                let (id, account_id, sequence, score, payload) = connection.query_row(
                    concat!(
                        "SELECT id, account_id, sequence, score, payload FROM ",
                        $table,
                        " WHERE id = ?1"
                    ),
                    [seed as i64],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, i64>(2)?,
                            row.get::<_, f64>(3)?,
                            row.get::<_, String>(4)?,
                        ))
                    },
                )?;
                let checksum = id as u64
                    ^ account_id as u64
                    ^ sequence as u64
                    ^ score.to_bits()
                    ^ payload.len() as u64;
                let indexed: i64 = connection.query_row(
                    concat!("SELECT count(*) FROM ", $table, " WHERE account_id = ?1"),
                    [(seed % 10_000) as i64],
                    |row| row.get(0),
                )?;
                Ok(checksum ^ indexed as u64)
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

fn main() -> rusqlite::Result<()> {
    let connection = Connection::open_in_memory()?;
    let mut checksum = table01::touch(&connection, 1)?;
    #[cfg(feature = "tables-2")]
    {
        checksum ^= table02::touch(&connection, 2)?;
    }
    #[cfg(feature = "tables-4")]
    {
        checksum ^= table03::touch(&connection, 3)? ^ table04::touch(&connection, 4)?;
    }
    #[cfg(feature = "tables-8")]
    {
        checksum ^= table05::touch(&connection, 5)?
            ^ table06::touch(&connection, 6)?
            ^ table07::touch(&connection, 7)?
            ^ table08::touch(&connection, 8)?;
    }
    println!("{checksum}");
    Ok(())
}
