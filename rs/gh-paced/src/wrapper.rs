//! One paced gh invocation: classify, guard, refresh the account snapshot, wait for admission,
//! run gh, and record the outcome.

use crate::audit::{self, Record};
use crate::budget::{halved, Bucket};
use crate::classify::{classify, Class, Classification};
use crate::clock::Clock;
use crate::config::{ClassLimits, Config};
use crate::guard::{self, Verdict};
use crate::pushback::{Pushback, Scanner};
use crate::ratelimit;
use crate::runner::{Exit, Invocation, Runner};
use crate::state::{self, Cooldown, Holder, Paths, State};
use crate::timefmt::human;
use std::path::PathBuf;

/// Exit status when a budget, cooldown or recursion limit refuses the call.
pub const EXIT_REFUSED: i32 = 75;
/// Exit status when the content guard refuses a write body.
pub const EXIT_CONTENT: i32 = 65;
/// Exit status for a usage error or a refused command shape (fast watch).
pub const EXIT_USAGE: i32 = 64;
/// Exit status for an internal error (state cannot be locked, read or written).
pub const EXIT_INTERNAL: i32 = 70;
/// Exit status when the real gh cannot be run.
pub const EXIT_NO_GH: i32 = 127;

/// How often a WRITE waiting for another in-flight WRITE re-checks, seconds.
pub const IN_FLIGHT_POLL_SECS: f64 = 2.0;
/// Shortest gap between repeated in-flight waiting warnings, seconds.
pub const IN_FLIGHT_WARN_SECS: f64 = 30.0;

/// How the invocation ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Exit with this status.
    Exit(i32),
    /// gh was killed by this signal; die by it too.
    Signal(i32),
}

/// Reads up to `n` bytes of stdin (until EOF or `n`).
pub type StdinReader<'a> = Box<dyn FnMut(usize) -> std::io::Result<Vec<u8>> + 'a>;

/// Everything one invocation needs. Tests build it with a fake clock and runner.
pub struct Wrapper<'a> {
    /// Time source.
    pub clock: &'a dyn Clock,
    /// Process runner.
    pub runner: &'a dyn Runner,
    /// Effective configuration.
    pub cfg: Config,
    /// State file paths for the account.
    pub paths: Paths,
    /// Short host name for messages and audit.
    pub host: String,
    /// Absolute path of the real gh.
    pub real_gh: PathBuf,
    /// Print messages to stderr as well as collecting them.
    pub echo: bool,
    /// Every message printed (for tests and for the caller).
    pub messages: Vec<String>,
    /// gh-paced's stdin is a terminal.
    pub stdin_is_tty: bool,
    /// Reads stdin when the content guard must inspect it.
    pub stdin_reader: StdinReader<'a>,
    /// Nonces of enclosing gh-paced invocations (`GH_PACED_INFLIGHT_CHAIN`).
    pub chain: Vec<String>,
    /// Nesting depth (`GH_PACED_DEPTH`).
    pub depth: u32,
    /// Liveness test for in-flight holders.
    pub alive: &'a dyn Fn(&Holder) -> bool,
    /// This process's ID.
    pub pid: u32,
    /// This process's start time in clock ticks.
    pub start_ticks: u64,
    /// This invocation's random identifier.
    pub nonce: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WaitKind {
    Cooldown,
    Block,
    InFlight,
    Bucket,
    Hour,
}

#[derive(Debug, Clone)]
struct Wait {
    secs: f64,
    kind: WaitKind,
    text: String,
}

fn fmt_num(x: f64) -> String {
    if (x - x.round()).abs() < 1e-9 {
        format!("{}", x.round() as i64)
    } else {
        format!("{x:.1}")
    }
}

/// Human description of a refill rate: `1 per 30 s` for slow rates, `20/min` otherwise.
pub fn describe_rate(l: ClassLimits) -> String {
    if l.per_minute <= 0.0 {
        return "0/min".to_string();
    }
    let interval = 60.0 / l.per_minute;
    if interval >= 10.0 - 1e-9 {
        format!("1 per {} s", fmt_num(interval))
    } else {
        format!("{}/min", fmt_num(l.per_minute))
    }
}

fn paced_api(class: Class) -> bool {
    matches!(class, Class::Read | Class::Search | Class::Write)
}

impl Wrapper<'_> {
    fn emit(&mut self, line: String) {
        if self.echo {
            eprintln!("{line}");
        }
        self.messages.push(line);
    }

    fn loud(&mut self, kind: &str, text: &str) {
        let line = format!("GH-PACED {kind} [{}] {text}", self.paths.account);
        self.emit(line);
    }

    fn banner(&mut self, kind: &str, lines: &[String]) {
        let rule = format!("GH-PACED {}", "*".repeat(68));
        self.emit(rule.clone());
        for l in lines {
            self.loud(kind, l);
        }
        self.emit(rule);
    }

    fn when(&self, at: f64) -> String {
        human(at, self.clock.now(), self.cfg.display_tz)
    }

    fn record(&self, event: &str, c: &Classification, summary: &str) -> Record {
        let mut r = Record::at(self.clock.now(), &self.host, event);
        r.class = c.class.name().to_string();
        r.cost = c.cost;
        r.command = c.command.clone();
        r.argv = summary.to_string();
        r
    }

    fn write_audit(&mut self, r: &Record) {
        if let Err(e) = audit::append(&self.paths.audit(), r) {
            self.loud("WARNING", &format!("audit log not written: {e}"));
        }
    }

    /// Lock, load and reap. Returns the guard and the state, or an error message.
    fn open_state(&mut self, now: f64) -> Result<(state::LockGuard, State), String> {
        let guard = state::lock(&self.paths)?;
        let loaded = state::load(&self.paths, now, &|t| {
            Class::PACED
                .iter()
                .map(|c| (c.name().to_string(), Bucket::empty(t)))
                .collect()
        });
        if let Some(w) = &loaded.warning {
            self.loud("WARNING", w);
        }
        let mut st = loaded.state;
        st.reap(self.alive);
        Ok((guard, st))
    }

    fn child_env(&self) -> Vec<(String, String)> {
        let mut chain = self.chain.clone();
        chain.push(self.nonce.clone());
        vec![
            ("GH_PACED_INFLIGHT_CHAIN".to_string(), chain.join(",")),
            ("GH_PACED_DEPTH".to_string(), (self.depth + 1).to_string()),
        ]
    }

    fn internal_error(&mut self, what: &str, e: &str) -> Outcome {
        self.banner(
            "ERROR",
            &[
                format!("{what}: {e}"),
                format!("refusing to run gh without its pacing state (exit {EXIT_INTERNAL})"),
            ],
        );
        Outcome::Exit(EXIT_INTERNAL)
    }

    /// Run one gh command line (without the program name).
    pub fn run(&mut self, args: &[String]) -> Outcome {
        let c = classify(args, &self.cfg);
        let rest = &args[c.rest_start.min(args.len())..];
        let mut shown: Vec<String> = args[..c.rest_start.min(args.len())].to_vec();
        shown.extend(guard::redact(&c, rest));
        let summary = audit::summarize(&shown);
        if c.class == Class::Local {
            return self.spawn(args, None, &c).0;
        }
        for w in c.warnings.clone() {
            self.loud("WARNING", &w);
        }
        if let Some(refusal) = c.refusal.clone() {
            self.banner(
                "REFUSED",
                &[
                    refusal.clone(),
                    format!("not running `{}` (exit {EXIT_USAGE})", c.command),
                ],
            );
            let mut r = self.record("refuse", &c, &summary);
            r.rc = Some(EXIT_USAGE);
            r.detail = refusal;
            if let Err(e) = self.audit_locked(&r) {
                return self.internal_error("cannot record refusal", &e);
            }
            return Outcome::Exit(EXIT_USAGE);
        }
        let stdin = match self.content_guard(&c, rest, &summary) {
            Ok(s) => s,
            Err(code) => return Outcome::Exit(code),
        };
        if let Err(e) = self.maybe_refresh(&c) {
            return self.internal_error("cannot refresh the rate-limit snapshot", &e);
        }
        let waited = match self.admit(&c, &summary) {
            Ok(w) => w,
            Err(code) => return Outcome::Exit(code),
        };
        let (outcome, scanner, exit) = self.spawn(args, stdin, &c);
        if let Err(e) = self.finish(&c, &summary, exit, &scanner, waited) {
            self.loud("ERROR", &format!("bookkeeping after gh exited failed: {e}"));
        }
        outcome
    }

    fn audit_locked(&mut self, r: &Record) -> Result<(), String> {
        let _guard = state::lock(&self.paths)?;
        self.write_audit(r);
        Ok(())
    }

    fn spawn(
        &mut self,
        args: &[String],
        stdin: Option<Vec<u8>>,
        c: &Classification,
    ) -> (Outcome, Scanner, Option<Exit>) {
        let mut scanner = Scanner::new();
        let env = if c.class == Class::Local {
            Vec::new()
        } else {
            self.child_env()
        };
        let real_gh = self.real_gh.clone();
        let inv = Invocation {
            program: &real_gh,
            args,
            stdin,
            env,
        };
        match self.runner.run(inv, &mut scanner) {
            Ok(Exit::Code(n)) => (Outcome::Exit(n), scanner, Some(Exit::Code(n))),
            Ok(Exit::Signal(s)) => (Outcome::Signal(s), scanner, Some(Exit::Signal(s))),
            Err(e) => {
                self.loud("ERROR", &e);
                (Outcome::Exit(EXIT_NO_GH), scanner, None)
            }
        }
    }

    fn content_guard(
        &mut self,
        c: &Classification,
        rest: &[String],
        summary: &str,
    ) -> Result<Option<Vec<u8>>, i32> {
        if c.class != Class::Write {
            return Ok(None);
        }
        if self.cfg.allow_large_body {
            self.loud(
                "WARNING",
                &format!(
                    "GH_PACED_ALLOW_LARGE_BODY=1: content guard skipped for `{}`",
                    c.command
                ),
            );
            return Ok(None);
        }
        let sources = guard::body_sources(c, rest, self.stdin_is_tty);
        let mut stdin_buf = None;
        if sources.iter().any(guard::BodySource::is_stdin) {
            match (self.stdin_reader)(self.cfg.max_body_bytes + 1) {
                Ok(buf) => stdin_buf = Some(buf),
                Err(e) => {
                    self.loud(
                        "REFUSED",
                        &format!("cannot read stdin for the content guard: {e}"),
                    );
                    return Err(EXIT_CONTENT);
                }
            }
        }
        match guard::evaluate(&sources, &self.cfg, stdin_buf.as_deref()) {
            Verdict::Allow { .. } => Ok(stdin_buf),
            Verdict::Refuse(reason) => {
                self.banner(
                    "REFUSED",
                    &[
                        format!("content guard: {reason}"),
                        format!("not running `{}` (exit {EXIT_CONTENT})", c.command),
                        "GitHub text is for short human notes. Keep evidence on the host and post a pointer \
                         (path + sha256, a tracked file, or a commit)."
                            .to_string(),
                    ],
                );
                let mut r = self.record("refuse", c, summary);
                r.rc = Some(EXIT_CONTENT);
                r.detail = format!("content guard: {reason}");
                if self.audit_locked(&r).is_err() {
                    self.loud("WARNING", "audit log not written for the content refusal");
                }
                Err(EXIT_CONTENT)
            }
        }
    }

    /// Refresh the account-wide snapshot when it is due, outside the lock.
    fn maybe_refresh(&mut self, c: &Classification) -> Result<(), String> {
        if !paced_api(c.class) {
            return Ok(());
        }
        if c.api.as_ref().is_some_and(|a| a.endpoint == "rate_limit") {
            return Ok(());
        }
        let now = self.clock.now();
        {
            let (_guard, mut st) = self.open_state(now)?;
            if st.cooldown.as_ref().is_some_and(|cd| cd.until > now) {
                return Ok(());
            }
            let age = st.rate_limit.as_ref().map(|s| now - s.fetched_at);
            let due = age.is_none_or(|a| a >= self.cfg.rate_limit_refresh_secs)
                || st.calls_since_refresh >= self.cfg.rate_limit_refresh_calls;
            if !due
                || now - st.last_refresh_attempt < self.cfg.rate_limit_min_refresh_secs
                || st.refresh_claim.is_some()
            {
                return Ok(());
            }
            st.refresh_claim = Some(Holder {
                pid: self.pid,
                start_ticks: self.start_ticks,
                nonce: self.nonce.clone(),
                class: Class::Read,
                since: now,
            });
            st.last_refresh_attempt = now;
            state::save(&self.paths, &st)?;
        }
        let args = vec!["api".to_string(), "rate_limit".to_string()];
        let real_gh = self.real_gh.clone();
        let result = self.runner.capture(
            Invocation {
                program: &real_gh,
                args: &args,
                stdin: None,
                env: self.child_env(),
            },
            self.cfg.rate_limit_timeout_secs,
        );
        let now = self.clock.now();
        let (_guard, mut st) = self.open_state(now)?;
        if st
            .refresh_claim
            .as_ref()
            .is_some_and(|h| h.nonce == self.nonce)
        {
            st.refresh_claim = None;
        }
        let rc_class = Classification {
            class: Class::Read,
            cost: 1,
            family: "api".into(),
            sub: None,
            command: "api GET rate_limit".into(),
            reason: "rate-limit refresh".into(),
            warnings: Vec::new(),
            refusal: None,
            api: None,
            rest_start: 1,
        };
        let mut r = self.record("refresh", &rc_class, "api rate_limit");
        match result {
            Ok(cap) => {
                let read = self.cfg.read;
                let bucket = st
                    .buckets
                    .entry(Class::Read.name().to_string())
                    .or_insert_with(|| Bucket::full(read, now));
                bucket.refresh(read, now);
                bucket.charge(1, now);
                let mut sc = Scanner::new();
                sc.feed(&cap.stderr);
                if let Some(pb) = sc.verdict(&self.cfg) {
                    self.apply_pushback(&mut st, &pb, "api GET rate_limit (gh-paced refresh)", now);
                    r.detail = format!("pushback: {}", pb.reason);
                } else if cap.exit == Exit::Code(0) && !cap.timed_out {
                    match ratelimit::parse(&cap.stdout, now) {
                        Ok(snap) => {
                            if let Some(p) = snap.pressure(Class::Read, now) {
                                r.detail = format!(
                                    "{} {}/{} left ({:.0}%)",
                                    p.resource,
                                    p.remaining,
                                    p.limit,
                                    p.fraction * 100.0
                                );
                            }
                            st.rate_limit = Some(snap);
                            st.calls_since_refresh = 0;
                        }
                        Err(e) => {
                            r.detail = format!("unparseable: {e}");
                            self.loud(
                                "WARNING",
                                &format!("rate-limit refresh output unusable ({e}); pacing on local budgets only"),
                            );
                        }
                    }
                } else {
                    let why = if cap.timed_out {
                        format!(
                            "timed out after {} s",
                            fmt_num(self.cfg.rate_limit_timeout_secs)
                        )
                    } else {
                        format!("exit {:?}", cap.exit)
                    };
                    r.detail = format!("failed: {why}");
                    self.loud(
                        "WARNING",
                        &format!("rate-limit refresh failed ({why}); pacing on local budgets only"),
                    );
                }
            }
            Err(e) => {
                r.detail = format!("failed: {e}");
                self.loud(
                    "WARNING",
                    &format!(
                        "rate-limit refresh could not start ({e}); pacing on local budgets only"
                    ),
                );
            }
        }
        self.write_audit(&r);
        state::save(&self.paths, &st)
    }

    fn apply_pushback(&mut self, st: &mut State, pb: &Pushback, command: &str, now: f64) {
        let until = now + pb.cooldown_secs;
        let extended = match &st.cooldown {
            Some(cd) if cd.until >= until => false,
            _ => {
                st.cooldown = Some(Cooldown {
                    until,
                    set_at: now,
                    reason: pb.reason.clone(),
                    command: command.to_string(),
                });
                true
            }
        };
        st.rate_limit = None;
        let effective = st.cooldown.as_ref().map(|c| c.until).unwrap_or(until);
        self.banner(
            "PUSHBACK",
            &[
                format!("GitHub refused or throttled `{command}`: {}", pb.reason),
                format!(
                    "every paced gh call for this account on this host is paused for {} s, until {}{}",
                    (effective - now).ceil() as i64,
                    self.when(effective),
                    if extended { "" } else { " (an existing longer cooldown stays)" }
                ),
                "STOP making GitHub calls and tell your coordinator. Do not retry in a loop. \
                 Never switch accounts to continue."
                    .to_string(),
            ],
        );
    }

    /// Wait until the call fits every budget, then charge it. Returns seconds waited.
    fn admit(&mut self, c: &Classification, summary: &str) -> Result<f64, i32> {
        let class = c.class;
        let base = self.cfg.limits(class);
        let mut waited = 0.0;
        let mut last_in_flight_warn: Option<f64> = None;
        let mut halve_noted = false;
        loop {
            let now = self.clock.now();
            let (guard, mut st) = match self.open_state(now) {
                Ok(x) => x,
                Err(e) => {
                    self.internal_error("cannot open pacing state", &e);
                    return Err(EXIT_INTERNAL);
                }
            };
            let pressure = if paced_api(class) {
                st.rate_limit.as_ref().and_then(|s| s.pressure(class, now))
            } else {
                None
            };
            let halve = pressure
                .as_ref()
                .is_some_and(|p| p.fraction <= self.cfg.halve_below_fraction);
            let limits = if halve { halved(base) } else { base };
            if halve && !halve_noted {
                if let Some(p) = &pressure {
                    let text = format!(
                        "account-wide {} budget at {:.0}% ({}/{} left until {}); local {} rates halved to {} and {}/hour",
                        p.resource,
                        p.fraction * 100.0,
                        p.remaining,
                        p.limit,
                        self.when(p.reset),
                        class.name(),
                        describe_rate(limits),
                        limits.per_hour
                    );
                    self.loud("WARNING", &text);
                }
                halve_noted = true;
            }
            let mut waits: Vec<Wait> = Vec::new();
            if let Some(cd) = &st.cooldown {
                if cd.until > now {
                    waits.push(Wait {
                        secs: cd.until - now,
                        kind: WaitKind::Cooldown,
                        text: format!(
                            "cooldown after GitHub pushback ({}) on `{}`",
                            cd.reason, cd.command
                        ),
                    });
                }
            }
            if let Some(p) = &pressure {
                if p.fraction <= self.cfg.block_below_fraction {
                    waits.push(Wait {
                        secs: (p.reset - now).max(1.0),
                        kind: WaitKind::Block,
                        text: format!(
                            "account-wide {} budget at {:.0}% ({}/{} left), at or below the {:.0}% floor; blocking until it resets",
                            p.resource,
                            p.fraction * 100.0,
                            p.remaining,
                            p.limit,
                            self.cfg.block_below_fraction * 100.0
                        ),
                    });
                }
            }
            if class == Class::Write {
                let others: Vec<u32> = st
                    .in_flight
                    .iter()
                    .filter(|h| h.class == Class::Write && !self.chain.contains(&h.nonce))
                    .map(|h| h.pid)
                    .collect();
                if u32::try_from(others.len()).unwrap_or(u32::MAX) >= self.cfg.write_max_in_flight {
                    let pids: Vec<String> = others.iter().map(u32::to_string).collect();
                    waits.push(Wait {
                        secs: IN_FLIGHT_POLL_SECS,
                        kind: WaitKind::InFlight,
                        text: format!(
                            "{} write(s) already in flight on this host (pid {}), limit {}",
                            others.len(),
                            pids.join(", "),
                            self.cfg.write_max_in_flight
                        ),
                    });
                }
            }
            let bucket = st
                .buckets
                .entry(class.name().to_string())
                .or_insert_with(|| Bucket::full(base, now));
            bucket.refresh(limits, now);
            let bw = bucket.bucket_wait(limits, c.cost);
            if bw > 0.0 {
                waits.push(Wait {
                    secs: bw,
                    kind: WaitKind::Bucket,
                    text: format!(
                        "{} budget {} (burst {}) per host reached",
                        class.name(),
                        describe_rate(limits),
                        fmt_num(limits.burst)
                    ),
                });
            }
            let hw = bucket.hour_wait(limits, c.cost, now);
            if hw > 0.0 {
                waits.push(Wait {
                    secs: hw,
                    kind: WaitKind::Hour,
                    text: format!(
                        "{} budget {}/hour per host reached ({} used in the last hour)",
                        class.name(),
                        limits.per_hour,
                        bucket.hour_used()
                    ),
                });
            }
            let worst = waits
                .iter()
                .max_by(|a, b| a.secs.total_cmp(&b.secs))
                .cloned();
            let Some(w) = worst else {
                bucket.charge(c.cost, now);
                if class == Class::Write {
                    st.in_flight.push(Holder {
                        pid: self.pid,
                        start_ticks: self.start_ticks,
                        nonce: self.nonce.clone(),
                        class,
                        since: now,
                    });
                }
                if class != Class::GitCredential {
                    st.calls_since_refresh = st.calls_since_refresh.saturating_add(1);
                }
                if let Err(e) = state::save(&self.paths, &st) {
                    drop(guard);
                    self.internal_error("cannot save pacing state", &e);
                    return Err(EXIT_INTERNAL);
                }
                let mut r = self.record("admit", c, summary);
                r.waited_secs = waited;
                self.write_audit(&r);
                return Ok(waited);
            };
            if let Err(e) = state::save(&self.paths, &st) {
                drop(guard);
                self.internal_error("cannot save pacing state", &e);
                return Err(EXIT_INTERNAL);
            }
            let max_wait = self.cfg.max_wait_secs;
            if waited + w.secs > max_wait {
                let until = now + w.secs;
                let wait_text = if w.kind == WaitKind::InFlight {
                    format!("waited {} s for it", waited.ceil() as i64)
                } else {
                    format!(
                        "the next slot is in {} s (at {})",
                        w.secs.ceil() as i64,
                        self.when(until)
                    )
                };
                self.banner(
                    "REFUSED",
                    &[
                        w.text.clone(),
                        format!(
                            "{wait_text}, beyond GH_PACED_MAX_WAIT={} s",
                            fmt_num(max_wait)
                        ),
                        format!("not running `{}` (exit {EXIT_REFUSED})", c.command),
                    ],
                );
                let mut r = self.record("refuse", c, summary);
                r.rc = Some(EXIT_REFUSED);
                r.waited_secs = waited;
                r.detail = w.text.clone();
                self.write_audit(&r);
                return Err(EXIT_REFUSED);
            }
            let warn = match w.kind {
                WaitKind::InFlight => {
                    last_in_flight_warn.is_none_or(|t| now - t >= IN_FLIGHT_WARN_SECS)
                }
                _ => true,
            };
            if warn {
                if w.kind == WaitKind::InFlight {
                    last_in_flight_warn = Some(now);
                    self.loud(
                        "WARNING",
                        &format!(
                            "{}; waiting (re-checking every {} s)",
                            w.text,
                            fmt_num(IN_FLIGHT_POLL_SECS)
                        ),
                    );
                } else {
                    let text = format!(
                        "{}; sleeping {} s (next slot {})",
                        w.text,
                        w.secs.ceil() as i64,
                        self.when(now + w.secs)
                    );
                    self.loud("WARNING", &text);
                }
                let mut r = self.record("throttle", c, summary);
                r.waited_secs = waited;
                r.detail = w.text.clone();
                self.write_audit(&r);
            }
            drop(guard);
            let sleep_for = if w.kind == WaitKind::InFlight {
                IN_FLIGHT_POLL_SECS
            } else {
                w.secs
            };
            self.clock.sleep(sleep_for);
            waited += sleep_for;
        }
    }

    fn finish(
        &mut self,
        c: &Classification,
        summary: &str,
        exit: Option<Exit>,
        scanner: &Scanner,
        waited: f64,
    ) -> Result<(), String> {
        let now = self.clock.now();
        let (_guard, mut st) = self.open_state(now)?;
        let nonce = self.nonce.clone();
        st.in_flight.retain(|h| h.nonce != nonce);
        if let Some(pb) = scanner.verdict(&self.cfg) {
            self.apply_pushback(&mut st, &pb, &c.command, now);
            let mut r = self.record("pushback", c, summary);
            r.detail = pb.reason.clone();
            self.write_audit(&r);
        }
        let mut r = self.record("exit", c, summary);
        r.waited_secs = waited;
        match exit {
            Some(Exit::Code(n)) => r.rc = Some(n),
            Some(Exit::Signal(s)) => r.signal = Some(s),
            None => r.rc = Some(EXIT_NO_GH),
        }
        self.write_audit(&r);
        state::save(&self.paths, &st)
    }
}
