//! One paced gh invocation: classify, copy the files it sends, guard, wait for admission
//! (refreshing the account snapshot when due), run gh, and record the outcome.
//!
//! Every decision is made while holding the account lock and against a time read after the lock
//! was taken, so a process that waited for the lock cannot charge a slot in the past.

use crate::audit::{self, Record};
use crate::budget::{halved, Bucket};
use crate::classify::{classify, Class, Classification};
use crate::clock::Clock;
use crate::config::{ClassLimits, Config};
use crate::guard::{self, Verdict};
use crate::pushback::{Pushback, Scanner};
use crate::ratelimit;
use crate::runner::{Exit, Invocation, Ran, Runner};
use crate::snapshot;
use crate::state::{self, Cooldown, Holder, Lease, Paths, State};
use crate::timefmt::human;
use std::os::fd::RawFd;
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

/// How often a WRITE waiting for another in-flight WRITE re-checks, seconds. Every re-check
/// prints a warning and writes a `throttle` audit record.
pub const IN_FLIGHT_POLL_SECS: f64 = 5.0;
/// How often a call waiting for another process's rate-limit refresh re-checks, seconds.
pub const REFRESH_POLL_SECS: f64 = 1.0;
/// The `command` recorded on the pause set by [`recovery_state`].
pub const RECOVERY_COMMAND: &str = "(state recovery)";

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
    /// Liveness test for in-flight holders and refresh claims (production:
    /// `state::holder_alive_in`, the lease lock when there is one).
    pub alive: &'a dyn Fn(&Holder) -> bool,
    /// Device and inode of every regular file this process inherited open
    /// (`state::inherited_file_ids`). An in-flight WRITE is treated as this invocation's own
    /// ancestor, and not waited for, only when its nonce is in [`Wrapper::chain`] AND its lease
    /// file is in this list, which only a descendant of that WRITE's gh can arrange.
    pub inherited_leases: Vec<(u64, u64)>,
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
    Refresh,
    Bucket,
    Hour,
}

impl WaitKind {
    /// A polled wait re-checks every few seconds instead of sleeping until a known time.
    fn polled(self) -> bool {
        matches!(self, WaitKind::InFlight | WaitKind::Refresh)
    }
}

/// What the refresh step decided for this admission pass.
enum RefreshStep {
    /// Nothing to do: proceed with the snapshot there is (possibly none).
    Skip,
    /// This process claimed the refresh and charged its READ token; run it outside the lock.
    Run,
    /// There is no snapshot and another live process is fetching one: wait for it.
    WaitForOther,
}

/// A call that passed admission.
struct Admitted {
    waited: f64,
    lease: Option<Lease>,
}

/// State used in place of an unusable state file: every class's hourly window counted as full
/// (the lost history may have held a full hour of calls) and a pause of `cooldown_secs`.
pub fn recovery_state(cfg: &Config, now: f64) -> State {
    let mut st = State::new();
    for class in Class::PACED {
        st.buckets.insert(
            class.name().to_string(),
            Bucket::saturated(cfg.limits(class), now),
        );
    }
    st.cooldown = Some(Cooldown {
        until: now + cfg.cooldown_secs,
        set_at: now,
        reason: "the pacing state file was unusable and was quarantined".to_string(),
        command: RECOVERY_COMMAND.to_string(),
    });
    st
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

    fn holder(&self, class: Class, since: f64, lease: Option<String>) -> Holder {
        Holder {
            pid: self.pid,
            start_ticks: self.start_ticks,
            nonce: self.nonce.clone(),
            class,
            since,
            lease,
        }
    }

    /// True when `h` is the in-flight WRITE of an enclosing gh-paced (see
    /// [`Wrapper::inherited_leases`]).
    fn is_ancestor(&self, h: &Holder) -> bool {
        self.chain.contains(&h.nonce)
            && h.lease
                .as_deref()
                .filter(|n| self.paths.is_lease_name(n))
                .and_then(|n| state::file_id(&self.paths.dir.join(n)))
                .is_some_and(|id| self.inherited_leases.contains(&id))
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

    /// Lock, read the clock, load and reap (removing dead holders' lease files). The time is
    /// read after the lock is held, so it is never older than the state it is applied to.
    fn open_state(&mut self) -> Result<(state::LockGuard, State, f64), String> {
        let guard = state::lock(&self.paths)?;
        let now = self.clock.now();
        let cfg = self.cfg.clone();
        let loaded = state::load(&self.paths, now, &|t| recovery_state(&cfg, t));
        if let Some(w) = &loaded.warning {
            let w = w.clone();
            self.banner("WARNING", &[w]);
        }
        let mut st = loaded.state;
        for dead in st.reap(self.alive) {
            if let Some(name) = &dead.lease {
                state::remove_lease(&self.paths, name);
            }
        }
        Ok((guard, st, now))
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
        let c0 = classify(args, &self.cfg);
        if c0.class == Class::Local {
            return self.spawn(args, None, &c0, None).0;
        }
        let split = c0.rest_start.min(args.len());
        let mut shown: Vec<String> = args[..split].to_vec();
        shown.extend(guard::redact(&c0, &args[split..]));
        let summary = audit::summarize(&shown);
        // Copy every file the command sends, and inspect and send only the copies.
        let (_snapshot, args) = match self.take_snapshot(&c0, args, split, &summary) {
            Ok(x) => x,
            Err(code) => return Outcome::Exit(code),
        };
        let args = args.as_slice();
        let c = classify(args, &self.cfg);
        if c.class == Class::Local {
            return self.spawn(args, None, &c, None).0;
        }
        let rest = &args[c.rest_start.min(args.len())..];
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
        let admitted = match self.admit(&c, &summary) {
            Ok(a) => a,
            Err(code) => return Outcome::Exit(code),
        };
        let keep_fd = admitted.lease.as_ref().map(Lease::fd);
        let (outcome, scanner, ran) = self.spawn(args, stdin, &c, keep_fd);
        if let Err(e) = self.finish(&c, &summary, ran, &scanner, admitted) {
            self.loud("ERROR", &format!("bookkeeping after gh exited failed: {e}"));
        }
        if ran.is_some_and(|r| r.deadline_hit) {
            return Outcome::Exit(EXIT_REFUSED);
        }
        outcome
    }

    /// Copy the files `c0` sends (see [`snapshot`]) and return the argument list naming the
    /// copies. Nothing is created when the command sends no file.
    fn take_snapshot(
        &mut self,
        c0: &Classification,
        args: &[String],
        split: usize,
        summary: &str,
    ) -> Result<(snapshot::Snapshot, Vec<String>), i32> {
        let rest0 = &args[split..];
        let files = guard::file_sources(c0, rest0);
        if files.is_empty() {
            return Ok((snapshot::Snapshot::default(), args.to_vec()));
        }
        let taken = state::ensure_dir(&self.paths.dir).and_then(|()| {
            snapshot::sweep(&self.paths.dir);
            snapshot::take(&self.paths.dir, &self.nonce, &files, rest0)
        });
        match taken {
            Ok((snap, new_rest)) => {
                let mut out = args[..split].to_vec();
                out.extend(new_rest);
                Ok((snap, out))
            }
            Err(e) => {
                let reason = format!("cannot copy a file the command sends for inspection: {e}");
                self.refuse_content(c0, summary, &reason);
                Err(EXIT_CONTENT)
            }
        }
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
        keep_fd: Option<RawFd>,
    ) -> (Outcome, Scanner, Option<Ran>) {
        let mut scanner = Scanner::new();
        let local = c.class == Class::Local;
        let env = if local { Vec::new() } else { self.child_env() };
        let real_gh = self.real_gh.clone();
        let inv = Invocation {
            program: &real_gh,
            args,
            stdin,
            env,
            deadline_secs: if local { None } else { c.deadline_secs },
            keep_fd,
            scan_stdout: !local && c.api.as_ref().is_some_and(|a| a.include),
        };
        match self.runner.run(inv, &mut scanner) {
            Ok(ran) => {
                let outcome = match ran.exit {
                    Exit::Code(n) => Outcome::Exit(n),
                    Exit::Signal(s) => Outcome::Signal(s),
                };
                (outcome, scanner, Some(ran))
            }
            Err(e) => {
                self.loud("ERROR", &e);
                (Outcome::Exit(EXIT_NO_GH), scanner, None)
            }
        }
    }

    /// Print and audit a content-guard refusal.
    fn refuse_content(&mut self, c: &Classification, summary: &str, reason: &str) {
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
        if let Some(why) = guard::uninspectable(c, rest, self.stdin_is_tty) {
            let reason = format!(
                "{why}, after this guard would have run, so the body cannot be inspected; \
                 pass it with --body or --body-file instead"
            );
            self.refuse_content(c, summary, &reason);
            return Err(EXIT_CONTENT);
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
                self.refuse_content(c, summary, &reason);
                Err(EXIT_CONTENT)
            }
        }
    }

    /// Decide, under the lock, whether this pass refreshes the account-wide snapshot. A refresh
    /// is due when the snapshot is missing, older than `rate_limit_refresh_secs`, or
    /// `rate_limit_refresh_calls` paced calls old; it is never attempted during a cooldown, more
    /// often than `rate_limit_min_refresh_secs`, while another live process holds the claim, or
    /// when the READ budget has no token for it. Claiming charges that READ token at once.
    fn refresh_step(&mut self, c: &Classification, st: &mut State, now: f64) -> RefreshStep {
        if !paced_api(c.class) || c.api.as_ref().is_some_and(|a| a.endpoint == "rate_limit") {
            return RefreshStep::Skip;
        }
        if st.cooldown.as_ref().is_some_and(|cd| cd.until > now) {
            return RefreshStep::Skip;
        }
        let age = st.rate_limit.as_ref().map(|s| now - s.fetched_at);
        let due = age.is_none_or(|a| a >= self.cfg.rate_limit_refresh_secs)
            || st.calls_since_refresh >= self.cfg.rate_limit_refresh_calls;
        if !due {
            return RefreshStep::Skip;
        }
        if st.refresh_claim.is_some() {
            return if st.rate_limit.is_none() {
                RefreshStep::WaitForOther
            } else {
                RefreshStep::Skip
            };
        }
        if now - st.last_refresh_attempt < self.cfg.rate_limit_min_refresh_secs {
            return RefreshStep::Skip;
        }
        let base = self.cfg.read;
        let limits = match st
            .rate_limit
            .as_ref()
            .and_then(|s| s.pressure(Class::Read, now))
        {
            Some(p) if p.fraction <= self.cfg.halve_below_fraction => halved(base),
            _ => base,
        };
        let bucket = st
            .buckets
            .entry(Class::Read.name().to_string())
            .or_insert_with(|| Bucket::full(base, now));
        bucket.refresh(limits, now);
        if bucket.bucket_wait(limits, 1) > 0.0 || bucket.hour_wait(limits, 1, now) > 0.0 {
            return RefreshStep::Skip;
        }
        bucket.charge(1, now);
        st.refresh_claim = Some(self.holder(Class::Read, now, None));
        st.last_refresh_attempt = now;
        RefreshStep::Run
    }

    /// Run the claimed refresh (outside the lock), then record its result under the lock.
    fn run_refresh(&mut self) -> Result<(), String> {
        let args = vec!["api".to_string(), "rate_limit".to_string()];
        let real_gh = self.real_gh.clone();
        let result = self.runner.capture(
            Invocation {
                program: &real_gh,
                args: &args,
                stdin: None,
                env: self.child_env(),
                deadline_secs: None,
                keep_fd: None,
                scan_stdout: false,
            },
            self.cfg.rate_limit_timeout_secs,
        );
        let (_guard, mut st, now) = self.open_state()?;
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
            deadline_secs: None,
        };
        let mut r = self.record("refresh", &rc_class, "api rate_limit");
        match result {
            Ok(cap) => {
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

    /// Wait until the call fits every budget, then charge it. A WRITE also takes the in-flight
    /// slot and its lease. Each pass takes the lock, reads the clock, refreshes the account
    /// snapshot first when that is due, and either admits or sleeps for the longest wait.
    fn admit(&mut self, c: &Classification, summary: &str) -> Result<Admitted, i32> {
        let class = c.class;
        let base = self.cfg.limits(class);
        let mut waited = 0.0;
        let mut halve_noted = false;
        loop {
            let (guard, mut st, now) = match self.open_state() {
                Ok(x) => x,
                Err(e) => {
                    self.internal_error("cannot open pacing state", &e);
                    return Err(EXIT_INTERNAL);
                }
            };
            let mut waits: Vec<Wait> = Vec::new();
            match self.refresh_step(c, &mut st, now) {
                RefreshStep::Run => {
                    let saved = state::save(&self.paths, &st);
                    drop(guard);
                    if let Err(e) = saved.and_then(|()| self.run_refresh()) {
                        self.internal_error("cannot refresh the rate-limit snapshot", &e);
                        return Err(EXIT_INTERNAL);
                    }
                    continue;
                }
                RefreshStep::WaitForOther => waits.push(Wait {
                    secs: REFRESH_POLL_SECS,
                    kind: WaitKind::Refresh,
                    text: "another gh-paced process is fetching the account-wide rate-limit \
                           snapshot and there is none yet"
                        .to_string(),
                }),
                RefreshStep::Skip => {}
            }
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
            if let Some(cd) = &st.cooldown {
                if cd.until > now {
                    let text = if cd.command == RECOVERY_COMMAND {
                        format!("pause after state recovery: {}", cd.reason)
                    } else {
                        format!(
                            "cooldown after GitHub pushback ({}) on `{}`",
                            cd.reason, cd.command
                        )
                    };
                    waits.push(Wait {
                        secs: cd.until - now,
                        kind: WaitKind::Cooldown,
                        text,
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
                    .filter(|h| h.class == Class::Write && !self.is_ancestor(h))
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
                let lease = if class == Class::Write {
                    match state::create_lease(&self.paths, &self.nonce) {
                        Ok(l) => Some(l),
                        Err(e) => {
                            drop(guard);
                            self.internal_error("cannot take the write lease", &e);
                            return Err(EXIT_INTERNAL);
                        }
                    }
                } else {
                    None
                };
                bucket.charge(c.cost, now);
                if let Some(l) = &lease {
                    let h = self.holder(class, now, Some(l.name.clone()));
                    st.in_flight.push(h);
                }
                if class != Class::GitCredential {
                    st.calls_since_refresh = st.calls_since_refresh.saturating_add(1);
                }
                if let Err(e) = state::save(&self.paths, &st) {
                    if let Some(l) = lease {
                        state::remove_lease(&self.paths, &l.name);
                    }
                    drop(guard);
                    self.internal_error("cannot save pacing state", &e);
                    return Err(EXIT_INTERNAL);
                }
                let mut r = self.record("admit", c, summary);
                r.waited_secs = waited;
                self.write_audit(&r);
                return Ok(Admitted { waited, lease });
            };
            if let Err(e) = state::save(&self.paths, &st) {
                drop(guard);
                self.internal_error("cannot save pacing state", &e);
                return Err(EXIT_INTERNAL);
            }
            if !w.secs.is_finite() {
                let text = format!(
                    "`{}` costs {} {} tokens, more than the whole hourly cap of {}{}; it can never be admitted",
                    c.command,
                    c.cost,
                    class.name(),
                    limits.per_hour,
                    if halve {
                        " (halved because the account-wide budget is low)"
                    } else {
                        ""
                    }
                );
                self.banner(
                    "REFUSED",
                    &[
                        text.clone(),
                        "lower --limit, drop --paginate, or split the work into smaller calls"
                            .to_string(),
                        format!("not running `{}` (exit {EXIT_REFUSED})", c.command),
                    ],
                );
                let mut r = self.record("refuse", c, summary);
                r.rc = Some(EXIT_REFUSED);
                r.waited_secs = waited;
                r.detail = text;
                self.write_audit(&r);
                return Err(EXIT_REFUSED);
            }
            let max_wait = self.cfg.max_wait_secs;
            if waited + w.secs > max_wait {
                let until = now + w.secs;
                let wait_text = if w.kind.polled() {
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
            let text = if w.kind.polled() {
                format!(
                    "{}; waiting (re-checking every {} s, waited {} s so far)",
                    w.text,
                    fmt_num(w.secs),
                    waited.ceil() as i64
                )
            } else {
                format!(
                    "{}; sleeping {} s (next slot {})",
                    w.text,
                    w.secs.ceil() as i64,
                    self.when(now + w.secs)
                )
            };
            self.loud("WARNING", &text);
            let mut r = self.record("throttle", c, summary);
            r.waited_secs = waited;
            r.detail = w.text.clone();
            self.write_audit(&r);
            drop(guard);
            self.clock.sleep(w.secs);
            waited += w.secs;
        }
    }

    fn finish(
        &mut self,
        c: &Classification,
        summary: &str,
        ran: Option<Ran>,
        scanner: &Scanner,
        admitted: Admitted,
    ) -> Result<(), String> {
        let (_guard, mut st, now) = self.open_state()?;
        let nonce = self.nonce.clone();
        st.in_flight.retain(|h| h.nonce != nonce);
        if let Some(lease) = admitted.lease {
            state::remove_lease(&self.paths, &lease.name);
        }
        if let Some(pb) = scanner.verdict(&self.cfg) {
            self.apply_pushback(&mut st, &pb, &c.command, now);
            let mut r = self.record("pushback", c, summary);
            r.detail = pb.reason.clone();
            self.write_audit(&r);
        }
        if let Some(limit) = c
            .deadline_secs
            .filter(|_| ran.is_some_and(|r| r.deadline_hit))
        {
            let text = format!(
                "`{}` ran past its {} s budget (its cost of {} tokens covers that many polls at \
                 the interval given) and was stopped",
                c.command,
                limit.ceil() as i64,
                c.cost
            );
            self.banner(
                "DEADLINE",
                &[
                    text.clone(),
                    format!("exit {EXIT_REFUSED}; re-run it if it still needs watching"),
                ],
            );
            let mut r = self.record("deadline", c, summary);
            r.detail = text;
            r.rc = Some(EXIT_REFUSED);
            self.write_audit(&r);
        }
        let mut r = self.record("exit", c, summary);
        r.waited_secs = admitted.waited;
        match ran.map(|r| r.exit) {
            Some(Exit::Code(n)) => r.rc = Some(n),
            Some(Exit::Signal(s)) => r.signal = Some(s),
            None => r.rc = Some(EXIT_NO_GH),
        }
        self.write_audit(&r);
        state::save(&self.paths, &st)
    }
}
