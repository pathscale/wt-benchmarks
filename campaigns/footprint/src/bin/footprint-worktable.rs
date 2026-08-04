macro_rules! define_table {
    ($module:ident, $name:ident, $table_type:ident, $row_type:ident) => {
        mod $module {
            use std::hint::black_box;
            use worktable::prelude::*;
            use worktable::worktable;

            worktable!(
                name: $name,
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
            );

            pub fn touch(seed: u64) -> u64 {
                let table = $table_type::default();
                table
                    .insert($row_type {
                        id: seed,
                        account_id: seed % 10_000,
                        sequence: seed.wrapping_mul(17),
                        score: seed as f64 / 100.0,
                        payload: "payloadpayload".to_string(),
                    })
                    .expect("fresh key");
                let row = black_box(table.select(seed)).expect("inserted row");
                let indexed = table
                    .select_by_account_id(row.account_id)
                    .execute()
                    .expect("secondary-index lookup");
                row.id
                    ^ row.account_id
                    ^ row.sequence
                    ^ row.score.to_bits()
                    ^ row.payload.len() as u64
                    ^ indexed.len() as u64
            }
        }
    };
}

define_table!(table01, Footprint01, Footprint01WorkTable, Footprint01Row);
#[cfg(feature = "tables-2")]
define_table!(table02, Footprint02, Footprint02WorkTable, Footprint02Row);
#[cfg(feature = "tables-4")]
define_table!(table03, Footprint03, Footprint03WorkTable, Footprint03Row);
#[cfg(feature = "tables-4")]
define_table!(table04, Footprint04, Footprint04WorkTable, Footprint04Row);
#[cfg(feature = "tables-8")]
define_table!(table05, Footprint05, Footprint05WorkTable, Footprint05Row);
#[cfg(feature = "tables-8")]
define_table!(table06, Footprint06, Footprint06WorkTable, Footprint06Row);
#[cfg(feature = "tables-8")]
define_table!(table07, Footprint07, Footprint07WorkTable, Footprint07Row);
#[cfg(feature = "tables-8")]
define_table!(table08, Footprint08, Footprint08WorkTable, Footprint08Row);

fn main() {
    let mut checksum = table01::touch(1);
    #[cfg(feature = "tables-2")]
    {
        checksum ^= table02::touch(2);
    }
    #[cfg(feature = "tables-4")]
    {
        checksum ^= table03::touch(3) ^ table04::touch(4);
    }
    #[cfg(feature = "tables-8")]
    {
        checksum ^= table05::touch(5) ^ table06::touch(6) ^ table07::touch(7) ^ table08::touch(8);
    }
    println!("{checksum}");
}
