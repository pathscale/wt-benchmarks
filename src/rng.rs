#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    pub fn below(&mut self, upper: u64) -> u64 {
        if upper == 0 {
            return 0;
        }

        ((self.next_u64() as u128 * upper as u128) >> 64) as u64
    }

    pub fn unit_f64(&mut self) -> f64 {
        const SCALE: f64 = 1.0 / ((1_u64 << 53) as f64);
        ((self.next_u64() >> 11) as f64) * SCALE
    }
}

/// The seed the path benches shuffle their probe order with.
///
/// Lifted verbatim out of `benches/arctic_paths.rs` so `benches/probe_order.rs`
/// shuffles the identical population in the identical order. Two benches that
/// each roll their own shuffle cannot be read against each other, and probe
/// order is the largest single effect in that comparison.
pub const PROBE_SHUFFLE_SEED: u64 = 0x5eed_1eaf_c0ff_ee01;

/// Fisher-Yates over `items`, seeded, in place.
///
/// Uses `Rng::next_u64` modulo the bound rather than `Rng::below`. That is a
/// slightly biased mapping and it is deliberate: it is the mapping the original
/// inline shuffle used, so the two benches keep producing the same permutation.
/// The bias is irrelevant to a probe order and a different one would silently
/// move every number that has already been recorded against this seed.
pub fn shuffle_seeded<T>(items: &mut [T], seed: u64) {
    let mut rng = Rng::new(seed);
    for index in (1..items.len()).rev() {
        let target = (rng.next_u64() % (index as u64 + 1)) as usize;
        items.swap(index, target);
    }
}

pub fn mix64(mut value: u64) -> u64 {
    value ^= value >> 30;
    value = value.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_bounded() {
        let mut left = Rng::new(42);
        let mut right = Rng::new(42);
        for _ in 0..1_000 {
            assert_eq!(left.next_u64(), right.next_u64());
        }

        let mut bounded = Rng::new(42);
        assert!((0..1_000).all(|_| bounded.below(17) < 17));
    }

    /// The permutation must match the loop this was lifted from, byte for byte.
    /// If it drifts, every probe-order number recorded against this seed becomes
    /// incomparable with the ones before it, and nothing in the output says so.
    #[test]
    fn shuffle_matches_the_inline_loop_it_replaced() {
        let mut expected: Vec<usize> = (0..1_000).collect();
        let mut state: u64 = PROBE_SHUFFLE_SEED;
        for index in (1..expected.len()).rev() {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            expected.swap(index, (state % (index as u64 + 1)) as usize);
        }

        let mut actual: Vec<usize> = (0..1_000).collect();
        shuffle_seeded(&mut actual, PROBE_SHUFFLE_SEED);

        assert_eq!(actual, expected);
    }

    #[test]
    fn shuffle_is_a_permutation() {
        let mut items: Vec<usize> = (0..257).collect();
        shuffle_seeded(&mut items, PROBE_SHUFFLE_SEED);
        let mut sorted = items.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, (0..257).collect::<Vec<_>>());
        assert_ne!(items, sorted);
    }
}
