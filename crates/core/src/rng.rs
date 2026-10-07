//! Seeded random numbers for gameplay. The whole generator state is one 32-bit integer stored in the game state,
//! so saving, hashing and replaying a game includes it. The simulation never uses any other source of randomness.

/// xorshift32: small, fast and fully reproducible on every platform (integer maths only).
pub fn next_random(state: &mut i32) -> u32 {
    let mut x = *state as u32;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *state = x as i32;
    x
}

/// An integer in `[0, n)`. `n` must not be 0.
pub fn random_int(state: &mut i32, n: u32) -> u32 {
    next_random(state) % n
}

/// The generator's starting state for a seed. xorshift must not start at 0, and nearby seeds should diverge
/// quickly, so the seed is mixed first.
pub fn seed_state(seed: i32) -> i32 {
    let mut s = (seed as u32) ^ 0x9e37_79b9;
    s = (s ^ (s >> 16)).wrapping_mul(0x85eb_ca6b);
    s = (s ^ (s >> 13)).wrapping_mul(0xc2b2_ae35);
    s ^= s >> 16;
    if s == 0 { 1 } else { s as i32 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_sequence_and_never_stuck_at_zero() {
        let (mut a, mut b) = (seed_state(42), seed_state(42));
        for _ in 0..1000 {
            assert_eq!(next_random(&mut a), next_random(&mut b));
            assert_ne!(a, 0);
        }
        assert_ne!(seed_state(1), seed_state(2));
    }
}
