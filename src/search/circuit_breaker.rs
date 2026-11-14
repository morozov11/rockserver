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

/// Trip threshold and cooldown of one guarded dependency.
#[derive(Clone, Copy, Debug)]
pub struct CircuitBreakerPolicy {
    /// Consecutive transient failures before the breaker opens.
    pub failure_threshold: u8,
    /// How long an open breaker waits before admitting one probe call.
    pub cooldown: Duration,
}

impl CircuitBreakerPolicy {
    /// The default policy used by the structured-intent LLM client.
    pub const fn llm() -> Self {
        Self {
            failure_threshold: FAILURE_THRESHOLD,
            cooldown: COOLDOWN,
        }
    }

    /// The service contract policy used for neural-service clients
    /// (5 consecutive failures, 30 s cooldown, single half-open probe).
    pub const fn service_contract() -> Self {
        Self {
            failure_threshold: 5,
            cooldown: COOLDOWN,
        }
    }
}

/// Shared-state admission control over a flaky remote dependency.
pub struct CircuitBreaker {
    policy: CircuitBreakerPolicy,
    state: Mutex<State>,
}

/// Granted call admission; finish it exactly once with the outcome.
pub struct Permit<'a> {
    breaker: &'a CircuitBreaker,
    probe: bool,
    finished: bool,
}

impl Default for CircuitBreaker {
    /// Creates a breaker with the default LLM policy.
    fn default() -> Self {
        Self::with_policy(CircuitBreakerPolicy::llm())
    }
}

impl CircuitBreaker {
    /// Creates a breaker with the default LLM policy.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a breaker with an explicit policy.
    pub fn with_policy(policy: CircuitBreakerPolicy) -> Self {
        Self {
            policy,
            state: Mutex::new(State::Closed { failures: 0 }),
        }
    }

    /// Returns a permit when a call may run now, or `None` while open.
    pub fn admit(&self, now: Instant) -> Option<Permit<'_>> {
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
    /// Records the call outcome; only transient failures advance the breaker.
    pub fn finish(mut self, transient_failure: bool, now: Instant) {
        let policy = self.breaker.policy;
        let mut state = self.breaker.state.lock().unwrap();
        if self.probe {
            *state = if transient_failure {
                State::Open {
                    retry_at: now + policy.cooldown,
                }
            } else {
                State::Closed { failures: 0 }
            };
        } else if let State::Closed { failures } = &mut *state {
            if transient_failure {
                *failures += 1;
                if *failures >= policy.failure_threshold {
                    *state = State::Open {
                        retry_at: now + policy.cooldown,
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
            let cooldown = self.breaker.policy.cooldown;
            let mut state = self.breaker.state.lock().unwrap();
            if matches!(*state, State::Probe) {
                *state = State::Open {
                    retry_at: Instant::now() + cooldown,
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
