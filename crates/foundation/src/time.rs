use std::time::{Duration, Instant};

/// Monotonic clock reporting the time between successive ticks.
#[derive(Debug, Clone, Copy)]
pub struct Clock {
    last: Instant,
}

impl Clock {
    /// Starts a clock at the current instant.
    pub fn new() -> Self {
        Self {
            last: Instant::now(),
        }
    }

    /// Returns the time elapsed since the previous `tick` (or `new`).
    pub fn tick(&mut self) -> Duration {
        let now = Instant::now();
        let delta = now - self.last;
        self.last = now;
        delta
    }
}

impl Default for Clock {
    fn default() -> Self {
        Self::new()
    }
}

/// Monotonically increasing frame number.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FrameCounter(u64);

impl FrameCounter {
    /// Counter starting at frame 0.
    pub fn new() -> Self {
        Self(0)
    }

    /// Current frame number.
    pub fn get(self) -> u64 {
        self.0
    }

    /// Advances to the next frame and returns the new number.
    pub fn advance(&mut self) -> u64 {
        self.0 += 1;
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_is_monotonic() {
        let mut c = Clock::new();
        std::thread::sleep(Duration::from_millis(2));
        assert!(c.tick() >= Duration::from_millis(2));
        assert!(c.tick() < Duration::from_secs(1));
    }

    #[test]
    fn frame_counter_advances() {
        let mut f = FrameCounter::new();
        assert_eq!(f.get(), 0);
        assert_eq!(f.advance(), 1);
        assert_eq!(f.get(), 1);
    }
}
