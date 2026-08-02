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
}
