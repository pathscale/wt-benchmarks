use std::hint::black_box;

#[derive(Clone)]
struct Row {
    id: u64,
    account_id: u64,
    sequence: u64,
    score: f64,
    payload: String,
}

fn touch(seed: u64) -> u64 {
    let row = Row {
        id: seed,
        account_id: seed % 10_000,
        sequence: seed.wrapping_mul(17),
        score: seed as f64 / 100.0,
        payload: "payloadpayload".to_string(),
    };
    let row = black_box(row.clone());
    row.id ^ row.account_id ^ row.sequence ^ row.score.to_bits() ^ row.payload.len() as u64 ^ 1
}

fn main() {
    let mut checksum = touch(1);
    #[cfg(feature = "tables-2")]
    {
        checksum ^= touch(2);
    }
    #[cfg(feature = "tables-4")]
    {
        checksum ^= touch(3) ^ touch(4);
    }
    #[cfg(feature = "tables-8")]
    {
        checksum ^= touch(5) ^ touch(6) ^ touch(7) ^ touch(8);
    }
    println!("{checksum}");
}
