//! Integer maths helpers. The simulation must give identical results on every machine, so it avoids
//! floating-point functions such as `sqrt` and `sin`, whose last bits are not guaranteed to match across
//! platforms and compilers.

/// Floor of the square root of `n`, by Newton's method on integers.
pub fn isqrt(n: u64) -> u64 {
    if n < 2 {
        return n;
    }
    let mut x = n;
    let mut y = x / 2 + x % 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

#[cfg(test)]
mod tests {
    use super::isqrt;

    #[test]
    #[allow(clippy::disallowed_types)] // the check, not the simulation, uses floating point
    fn matches_floor_sqrt_on_squares_neighbours_and_a_sweep() {
        for r in 0u64..5_000 {
            assert_eq!(isqrt(r * r), r);
            if r > 0 {
                assert_eq!(isqrt(r * r - 1), r - 1);
            }
            assert_eq!(isqrt(r * r + r), r);
        }
        // Tests may use floating point to check the integer version; the simulation never does.
        for n in (0u64..200_000).step_by(7) {
            assert_eq!(isqrt(n), (n as f64).sqrt().floor() as u64);
        }
        assert_eq!(isqrt(1 << 52), 1 << 26);
        assert_eq!(isqrt(u64::MAX), u32::MAX as u64);
    }
}
