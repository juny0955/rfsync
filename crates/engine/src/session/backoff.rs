use std::time::Duration;

const BACKOFF_INITIAL: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);

pub struct Backoff(Duration);

impl Backoff {
    pub fn new() -> Self {
        Self(BACKOFF_INITIAL)
    }

    pub fn reset(&mut self) {
        self.0 = BACKOFF_INITIAL;
    }

    /// 이번에 기다릴 시간을 돌려주고, 다음 대기 시간을 두 배로 늘린다(상한까지).
    pub fn next_delay(&mut self) -> Duration {
        let current = self.0;
        self.0 = (current * 2).min(BACKOFF_MAX);
        current
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doubles_the_delay_until_the_maximum() {
        let mut backoff = Backoff::new();

        let delays: Vec<Duration> = (0..7).map(|_| backoff.next_delay()).collect();

        assert_eq!(delays, [1, 2, 4, 8, 16, 30, 30].map(Duration::from_secs));
    }

    #[test]
    fn starts_over_from_the_initial_delay_after_reset() {
        let mut backoff = Backoff::new();
        for _ in 0..4 {
            backoff.next_delay();
        }

        backoff.reset();

        assert_eq!(backoff.next_delay(), BACKOFF_INITIAL);
        assert_eq!(backoff.next_delay(), BACKOFF_INITIAL * 2);
    }
}
