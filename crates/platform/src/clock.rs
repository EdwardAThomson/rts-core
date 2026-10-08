//! A monotonic clock. `std::time::Instant` panics in the browser, so the web build reads `performance.now()`.

#[cfg(not(target_arch = "wasm32"))]
pub use std::time::Instant;

#[cfg(target_arch = "wasm32")]
pub use web::Instant;

#[cfg(target_arch = "wasm32")]
mod web {
    use std::time::Duration;

    /// A point in time, in milliseconds since the page loaded.
    #[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
    pub struct Instant(f64);

    impl Instant {
        pub fn now() -> Instant {
            Instant(web_sys::window().and_then(|w| w.performance()).map_or(0.0, |p| p.now()))
        }
    }

    impl std::ops::Sub for Instant {
        type Output = Duration;

        fn sub(self, earlier: Instant) -> Duration {
            Duration::from_secs_f64(((self.0 - earlier.0) / 1000.0).max(0.0))
        }
    }
}
