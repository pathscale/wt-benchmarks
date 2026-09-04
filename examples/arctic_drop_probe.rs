use std::time::Instant;

use arctic::ConcurrentMap;
use arctic::concurrent::smr::PsReclaim;
use wt_benchmarks::moe_pgo::{arctic as arctic_backend, congee, wti};

const WIDTH: u32 = 12_288;
const MAPS: usize = 256;

fn main() {
    let maps = (0..MAPS)
        .map(|_| {
            let map = ConcurrentMap::<u32, u64, PsReclaim>::new();
            for key in 0..WIDTH {
                map.insert(key, key.into()).unwrap();
            }
            map
        })
        .collect::<Vec<_>>();

    let start = Instant::now();
    drop(maps);
    let elapsed = start.elapsed();
    println!(
        "dropped {MAPS} Arctic maps x {WIDTH} entries in {:?}: {:.3} us/map, {:.3} ns/entry",
        elapsed,
        elapsed.as_secs_f64() * 1e6 / MAPS as f64,
        elapsed.as_secs_f64() * 1e9 / (MAPS as u32 * WIDTH) as f64,
    );

    macro_rules! table_drop {
        ($module:ident) => {{
            let tables = (0..64)
                .map(|_| {
                    let set = $module::build(WIDTH);
                    let table = set.remove(0).unwrap();
                    drop(set);
                    table
                })
                .collect::<Vec<_>>();
            let start = Instant::now();
            drop(tables);
            let elapsed = start.elapsed();
            println!(
                "dropped 64 {} WorkTables x {WIDTH} rows in {:?}: {:.3} us/table, {:.3} ns/row",
                stringify!($module),
                elapsed,
                elapsed.as_secs_f64() * 1e6 / 64.0,
                elapsed.as_secs_f64() * 1e9 / (64 * WIDTH as usize) as f64,
            );
        }};
    }

    table_drop!(wti);
    table_drop!(arctic_backend);
    table_drop!(congee);
}
