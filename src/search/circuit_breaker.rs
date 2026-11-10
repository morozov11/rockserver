//! Bounded admission for transient structured-intent provider outages.

use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

const FAILURE_THRESHOLD: u8 = 3;
const COOLDOWN: Duration = Duration::from_secs(30);

enum State {
    Closed { failures: u8 },
    Open { retry_at: Instant },
    Probe,
}

pub(super) struct CircuitBreaker {
    state: Mutex<State>,
}

pub(super) struct Permit<'a> {
    breaker: &'a CircuitBreaker,
    probe: bool,
    finished: bool,
}

impl CircuitBreaker {
    pub(super) fn new() -> Self {
        Self {
            state: Mutex::new(State::Closed { failures: 0 }),
        }
    }

    pub(super) fn admit(&self, now: Instant) -> Option<Permit<'_>> {
        let mut state = self.state.lock().unwrap();
        let probe = match *state {
            State::Closed { .. } => false,
            State::Open { retry_at } if now >= retry_at => {
                *state = State::Probe;
                true
            }
            State::Open { .. } | State::Probe => return None,
        };
        Some(Permit {
            breaker: self,
            probe,
            finished: false,
        })
    }
}

impl Permit<'_> {
    pub(super) fn finish(mut self, transient_failure: bool, now: Instant) {
        let mut state = self.breaker.state.lock().unwrap();
        if self.probe {
            *state = if transient_failure {
                State::Open {
                    retry_at: now + COOLDOWN,
                }
            } else {
                State::Closed { failures: 0 }
            };
        } else if let State::Closed { failures } = &mut *state {
            if transient_failure {
                *failures += 1;
                if *failures >= FAILURE_THRESHOLD {
                    *state = State::Open {
                        retry_at: now + COOLDOWN,
                    };
                }
            } else {
                *failures = 0;
            }
        }
        self.finished = true;
    }
}

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        // A cancelled probe must not leave the breaker half-open forever.
        if self.probe && !self.finished {
            let mut state = self.breaker.state.lock().unwrap();
            if matches!(*state, State::Probe) {
                *state = State::Open {
                    retry_at: Instant::now() + COOLDOWN,
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consecutive_transient_failures_open_and_single_probe_recovers() {
        let breaker = CircuitBreaker::new();
        let start = Instant::now();
        breaker.admit(start).unwrap().finish(true, start);
        breaker.admit(start).unwrap().finish(false, start);
        for _ in 0..FAILURE_THRESHOLD {
            breaker.admit(start).unwrap().finish(true, start);
        }
        assert!(
            breaker
                .admit(start + COOLDOWN - Duration::from_nanos(1))
                .is_none()
        );
        let probe = breaker.admit(start + COOLDOWN).unwrap();
        assert!(breaker.admit(start + COOLDOWN).is_none());
        probe.finish(false, start + COOLDOWN);
        assert!(breaker.admit(start + COOLDOWN).is_some());
    }

    #[test]
    fn failed_probe_restarts_cooldown() {
        let breaker = CircuitBreaker::new();
        let start = Instant::now();
        for _ in 0..FAILURE_THRESHOLD {
            breaker.admit(start).unwrap().finish(true, start);
        }
        breaker
            .admit(start + COOLDOWN)
            .unwrap()
            .finish(true, start + COOLDOWN);
        assert!(breaker.admit(start + COOLDOWN).is_none());
        assert!(breaker.admit(start + COOLDOWN * 2).is_some());
    }

    #[test]
    fn dropped_probe_reopens_instead_of_blocking_recovery() {
        let breaker = CircuitBreaker::new();
        let start = Instant::now();
        for _ in 0..FAILURE_THRESHOLD {
            breaker.admit(start).unwrap().finish(true, start);
        }
        drop(breaker.admit(start + COOLDOWN).unwrap());
        assert!(breaker.admit(Instant::now()).is_none());
        assert!(breaker.admit(Instant::now() + COOLDOWN).is_some());
    }
}
