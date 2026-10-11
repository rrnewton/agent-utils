//! A client-side ceiling on calls to a model provider: a token bucket with a waiting line.
//! `#233 llm-summarizer`.
//!
//! # The problem
//!
//! A thread list can ask for dozens of thread names at once, and a page full of long messages can
//! ask for a dozen summaries at once. The provider's own rate limit is the operator's to spend,
//! shared with whatever else uses the same key, and a 429 storm from this server would spend it on
//! retries. So calls are paced HERE, before they leave: at most `burst` back to back, then one per
//! `60 / requests_per_minute` seconds. A caller over the limit WAITS its turn rather than being
//! refused, up to a deadline it chooses; only a caller whose turn is past that deadline is turned
//! away, and it is told how long it would have waited.
//!
//! # A 429 is obeyed by everyone
//!
//! When the provider answers 429 with `Retry-After`, [`RateLimiter::pause_for`] stops EVERY caller
//! until then and leaves exactly one call for the moment it ends, so calls resume at the steady
//! pace rather than as a burst that would earn the next 429.
//!
//! # Interactive work goes first
//!
//! Thread names are produced in the background; a message summary is produced while somebody
//! waits for it. [`Priority::Background`] therefore takes a token only when one more would still be
//! left for an interactive caller (when the burst allows a spare at all), so a backlog of thread
//! names never makes a reader wait behind it for a whole refill.
//!
//! Time is `tokio::time`, so a test with a paused clock can assert exactly when each call went.

use std::time::Duration;

use tokio::time::Instant;

/// Who is asking, which decides whether a token must be left over for somebody else.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Priority {
    /// Somebody is waiting for this answer.
    Interactive,
    /// Nobody is waiting: a thread name refreshed in the background.
    Background,
}

/// The pace, and the burst before it applies.
#[derive(Debug)]
pub struct RateLimiter {
    per_second: f64,
    burst: f64,
    bucket: tokio::sync::Mutex<Bucket>,
}

#[derive(Debug)]
struct Bucket {
    tokens: f64,
    updated: Instant,
    paused_until: Option<Instant>,
}

impl RateLimiter {
    /// A limiter allowing `burst` calls at once and `per_minute` a minute after that. Both are
    /// clamped to at least one: a limiter that never admits anything is a configuration error,
    /// refused at load, not a state this type has to represent.
    #[must_use]
    pub fn new(per_minute: u32, burst: u32) -> Self {
        let burst = f64::from(burst.max(1));
        Self {
            per_second: f64::from(per_minute.max(1)) / 60.0,
            burst,
            bucket: tokio::sync::Mutex::new(Bucket {
                tokens: burst,
                updated: Instant::now(),
                paused_until: None,
            }),
        }
    }

    /// Wait for a turn, for at most `max_wait`.
    ///
    /// # Errors
    ///
    /// The wait that WOULD have been needed, when it is longer than `max_wait`. Nothing is
    /// reserved in that case, so a refused caller costs the others nothing.
    pub async fn acquire(&self, priority: Priority, max_wait: Duration) -> Result<(), Duration> {
        let deadline = Instant::now() + max_wait;
        loop {
            let wait = {
                let mut bucket = self.bucket.lock().await;
                let now = Instant::now();
                self.refill(&mut bucket, now);
                let reserve = match priority {
                    Priority::Interactive => 0.0,
                    Priority::Background if self.burst >= 2.0 => 1.0,
                    Priority::Background => 0.0,
                };
                let paused = bucket
                    .paused_until
                    .filter(|until| *until > now)
                    .map_or(Duration::ZERO, |until| until - now);
                if paused.is_zero() && bucket.tokens >= 1.0 + reserve {
                    bucket.tokens -= 1.0;
                    return Ok(());
                }
                let deficit = (1.0 + reserve - bucket.tokens).max(0.0);
                // Rounded UP to the millisecond, so a waiter never wakes a hair early, finds the
                // bucket a fraction short and has to sleep again for nothing.
                let refill =
                    Duration::from_millis((deficit / self.per_second * 1000.0).ceil() as u64);
                paused.max(refill)
            };
            if Instant::now() + wait > deadline {
                return Err(wait);
            }
            tokio::time::sleep(wait).await;
        }
    }

    /// Stop every caller for `wait`, and start again at the steady pace rather than with a burst.
    ///
    /// What a 429's `Retry-After` turns into. A pause already longer than this one is kept.
    pub async fn pause_for(&self, wait: Duration) {
        let mut bucket = self.bucket.lock().await;
        let now = Instant::now();
        self.refill(&mut bucket, now);
        let until = bucket
            .paused_until
            .map_or(now + wait, |held| held.max(now + wait));
        bucket.paused_until = Some(until);
        // ONE call may go the moment the pause ends — the retry the provider invited — and after
        // it the steady pace. Refill is counted from the END of the pause: tokens that "accrued"
        // while the provider was refusing us are not tokens we may spend.
        bucket.tokens = 1.0;
        bucket.updated = until;
    }

    fn refill(&self, bucket: &mut Bucket, now: Instant) {
        if now > bucket.updated {
            let elapsed = (now - bucket.updated).as_secs_f64();
            bucket.tokens = (bucket.tokens + elapsed * self.per_second).min(self.burst);
            bucket.updated = now;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    /// Every pair of grants (i, j) must satisfy: grants in [t_i, t_j] <= burst + rate * (t_j - t_i).
    /// That is the definition of "never exceeds the limit", stated without trusting the
    /// implementation's own arithmetic.
    fn assert_within_limit(grants: &[Duration], per_minute: u32, burst: u32) {
        let rate = f64::from(per_minute) / 60.0;
        for i in 0..grants.len() {
            for j in i..grants.len() {
                let window = (grants[j] - grants[i]).as_secs_f64();
                let allowed = f64::from(burst) + rate * window + 1e-6;
                let count = (j - i + 1) as f64;
                assert!(
                    count <= allowed,
                    "{count} calls went out in {window}s, more than the {allowed} the limit allows: \
                     {grants:?}"
                );
            }
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_burst_of_callers_is_queued_and_never_exceeds_the_limit() {
        let limiter = Arc::new(RateLimiter::new(15, 3));
        let start = Instant::now();
        let grants = Arc::new(Mutex::new(Vec::new()));
        let mut tasks = Vec::new();
        for _ in 0..12 {
            let limiter = Arc::clone(&limiter);
            let grants = Arc::clone(&grants);
            tasks.push(tokio::spawn(async move {
                limiter
                    .acquire(Priority::Interactive, Duration::from_secs(3600))
                    .await
                    .expect("a long deadline admits everyone eventually");
                grants.lock().unwrap().push(start.elapsed());
            }));
        }
        for task in tasks {
            task.await.unwrap();
        }
        let mut grants = grants.lock().unwrap().clone();
        grants.sort();
        assert_eq!(grants.len(), 12, "every caller got its turn");
        assert_within_limit(&grants, 15, 3);
        // The control: the burst really went out at once, and the queue really did drain at the
        // configured pace rather than at some slower one that would pass the check above trivially.
        assert!(grants[2] < Duration::from_millis(10), "{grants:?}");
        assert!(
            grants[11] <= Duration::from_secs(37),
            "nine calls past the burst at 4s apiece should take about 36s: {grants:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_caller_whose_turn_is_past_its_deadline_is_told_how_long_it_would_have_waited() {
        let limiter = RateLimiter::new(6, 1);
        limiter
            .acquire(Priority::Interactive, Duration::ZERO)
            .await
            .expect("the first is free");
        let refused = limiter
            .acquire(Priority::Interactive, Duration::from_secs(1))
            .await
            .expect_err("the next slot is ten seconds away");
        assert!(refused >= Duration::from_secs(9), "{refused:?}");
        // ...and the refusal reserved nothing: the next caller with patience goes at ten seconds.
        let start = Instant::now();
        limiter
            .acquire(Priority::Interactive, Duration::from_secs(60))
            .await
            .expect("admitted");
        assert!(
            start.elapsed() <= Duration::from_secs(10),
            "{:?}",
            start.elapsed()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_429_pause_holds_everyone_and_resumes_at_the_steady_pace() {
        let limiter = RateLimiter::new(60, 5);
        let start = Instant::now();
        limiter.pause_for(Duration::from_secs(30)).await;
        limiter
            .acquire(Priority::Interactive, Duration::from_secs(120))
            .await
            .expect("admitted after the pause");
        let first = start.elapsed();
        assert!(
            first >= Duration::from_secs(30),
            "the pause was not obeyed: {first:?}"
        );
        assert!(
            first < Duration::from_millis(30_100),
            "the retry the provider invited should go the moment the pause ends: {first:?}"
        );
        limiter
            .acquire(Priority::Interactive, Duration::from_secs(120))
            .await
            .expect("admitted");
        assert!(
            start.elapsed() - first >= Duration::from_millis(900),
            "the pause ended in a burst rather than at the steady pace"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn background_work_leaves_a_token_for_somebody_who_is_waiting() {
        let limiter = RateLimiter::new(60, 2);
        limiter
            .acquire(Priority::Background, Duration::ZERO)
            .await
            .expect("two tokens: background may take one");
        limiter
            .acquire(Priority::Background, Duration::ZERO)
            .await
            .expect_err("one token left: it is kept for an interactive caller");
        limiter
            .acquire(Priority::Interactive, Duration::ZERO)
            .await
            .expect("the kept token is there for an interactive caller");
    }
}
