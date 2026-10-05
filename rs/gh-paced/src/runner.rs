//! Running the real `gh`: transparent pass-through with stderr teed through the pushback scanner.
//!
//! The child inherits stdin (unless gh-paced had to buffer it for the content guard) and stdout
//! unchanged. Stderr goes through gh-paced so it can be scanned while it streams: through a
//! pseudo-terminal when gh-paced's own stderr is a terminal (so gh still sees a terminal and
//! keeps its colours, spinners and prompts), otherwise through a pipe. User-sent INT, TERM, HUP
//! and QUIT are forwarded to the child; terminal-generated signals already reach it because it
//! shares the foreground process group. A child killed by a signal makes gh-paced die by the
//! same signal after bookkeeping, so callers see the same status as from gh itself.
//!
//! For `gh api --include`, which prints the HTTP status line and response headers on stdout,
//! stdout is teed the same way (through a pseudo-terminal when gh-paced's stdout is a terminal)
//! and fed to the scanner's header parser, so a `Retry-After` or `x-ratelimit-remaining: 0`
//! header is seen. A command with a deadline (a watch) is sent SIGTERM when the deadline passes
//! and SIGKILL [`KILL_GRACE_SECS`] later. A write lease descriptor is left open across exec so the
//! lease lives as long as gh does.

use crate::pushback::Scanner;
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

/// How the child ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    /// Normal exit with this status.
    Code(i32),
    /// Killed by this signal.
    Signal(i32),
}

/// Seconds between SIGTERM and SIGKILL when a deadline passes.
pub const KILL_GRACE_SECS: f64 = 5.0;

/// One child process to run.
#[derive(Debug, Clone)]
pub struct Invocation<'a> {
    /// Absolute path of the real gh.
    pub program: &'a Path,
    /// Arguments after the program name.
    pub args: &'a [String],
    /// `Some(bytes)`: feed exactly these bytes on stdin (already read for the content guard).
    /// `None`: the child inherits stdin.
    pub stdin: Option<Vec<u8>>,
    /// Extra environment variables for the child.
    pub env: Vec<(String, String)>,
    /// Stop the child once it has run this many seconds (SIGTERM, then SIGKILL after
    /// [`KILL_GRACE_SECS`]). `None`: no limit.
    pub deadline_secs: Option<f64>,
    /// A descriptor the child keeps open across exec (the write lease).
    pub keep_fd: Option<RawFd>,
    /// Tee stdout through gh-paced and feed it to [`Scanner::feed_headers`].
    pub scan_stdout: bool,
}

/// How an interactive run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ran {
    /// The child's exit.
    pub exit: Exit,
    /// The child was stopped because its deadline passed.
    pub deadline_hit: bool,
}

/// Output of a captured (non-interactive) run.
#[derive(Debug, Clone)]
pub struct Captured {
    /// How the child ended (`Signal(9)` after a timeout kill).
    pub exit: Exit,
    /// Everything it wrote to stdout.
    pub stdout: Vec<u8>,
    /// Everything it wrote to stderr.
    pub stderr: Vec<u8>,
    /// The child was killed because it ran past the timeout.
    pub timed_out: bool,
}

/// Something that can run gh. Tests substitute a fake.
pub trait Runner: Send + Sync {
    /// Run interactively: stdout inherited (or teed, see [`Invocation::scan_stdout`]), stderr
    /// streamed to our stderr and fed to `scanner`.
    fn run(&self, inv: Invocation<'_>, scanner: &mut Scanner) -> Result<Ran, String>;
    /// Run with stdout and stderr captured, killing the child after `timeout_secs`.
    fn capture(&self, inv: Invocation<'_>, timeout_secs: f64) -> Result<Captured, String>;
}

/// Spawns the real gh.
#[derive(Debug, Default, Clone, Copy)]
pub struct RealRunner;

static CHILD_PID: AtomicI32 = AtomicI32::new(0);

const FORWARDED: [libc::c_int; 4] = [libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGQUIT];

extern "C" fn forward_signal(
    sig: libc::c_int,
    info: *mut libc::siginfo_t,
    _ctx: *mut libc::c_void,
) {
    // Only signals sent by a process (kill, sigqueue, tgkill: si_code <= 0) are forwarded.
    // Terminal-generated signals (si_code SI_KERNEL) already went to the whole foreground
    // process group, child included.
    // SAFETY: the kernel passes a valid siginfo_t to an SA_SIGINFO handler.
    let code = if info.is_null() {
        0
    } else {
        unsafe { (*info).si_code }
    };
    if code <= 0 {
        let pid = CHILD_PID.load(Ordering::SeqCst);
        if pid > 0 {
            // SAFETY: kill is async-signal-safe.
            unsafe {
                libc::kill(pid, sig);
            }
        }
    }
}

fn signal_set() -> libc::sigset_t {
    // SAFETY: sigemptyset/sigaddset initialise and fill a local sigset_t.
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        for s in FORWARDED {
            libc::sigaddset(&mut set, s);
        }
        set
    }
}

/// Install the forwarding handler for each signal in [`FORWARDED`] that is not ignored, and
/// report which ones got it. An ignored signal (for example SIGHUP under `nohup`) stays ignored,
/// so the child inherits the same disposition it would have had without gh-paced.
fn install_forwarders() -> [bool; FORWARDED.len()] {
    let mut installed = [false; FORWARDED.len()];
    for (i, s) in FORWARDED.into_iter().enumerate() {
        // SAFETY: querying the current disposition, then installing a handler with a valid
        // function pointer and an empty mask.
        unsafe {
            let mut current: libc::sigaction = std::mem::zeroed();
            if libc::sigaction(s, std::ptr::null(), &mut current) == 0
                && current.sa_sigaction == libc::SIG_IGN
            {
                continue;
            }
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = forward_signal as *const () as usize;
            action.sa_flags = libc::SA_SIGINFO | libc::SA_RESTART;
            libc::sigemptyset(&mut action.sa_mask);
            installed[i] = libc::sigaction(s, &action, std::ptr::null_mut()) == 0;
        }
    }
    installed
}

/// Die by `sig` (after resetting its handler), so the caller sees the child's status. Core dumps
/// are suppressed. Falls back to `exit(128 + sig)`.
pub fn die_by_signal(sig: i32) -> ! {
    let core = [
        libc::SIGQUIT,
        libc::SIGABRT,
        libc::SIGSEGV,
        libc::SIGBUS,
        libc::SIGFPE,
        libc::SIGILL,
        libc::SIGSYS,
        libc::SIGTRAP,
        libc::SIGXCPU,
        libc::SIGXFSZ,
    ];
    // SAFETY: plain libc calls on this process.
    unsafe {
        if core.contains(&sig) {
            let zero = libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            };
            libc::setrlimit(libc::RLIMIT_CORE, &zero);
        }
        libc::signal(sig, libc::SIG_DFL);
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, sig);
        libc::pthread_sigmask(libc::SIG_UNBLOCK, &set, std::ptr::null_mut());
        libc::raise(sig);
    }
    std::process::exit(128 + sig)
}

/// A pseudo-terminal pair: the master (read by gh-paced) and the slave (the child's stderr).
struct Pty {
    master: File,
    slave: File,
}

/// Open a pseudo-terminal whose window size is copied from descriptor `size_from`.
fn open_pty(size_from: RawFd) -> Option<Pty> {
    // SAFETY: standard pty allocation; every returned descriptor is checked and owned.
    unsafe {
        let master = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC);
        if master < 0 {
            return None;
        }
        let master_file = File::from_raw_fd(master);
        if libc::grantpt(master) != 0 || libc::unlockpt(master) != 0 {
            return None;
        }
        let mut name = [0 as libc::c_char; 128];
        if libc::ptsname_r(master, name.as_mut_ptr(), name.len()) != 0 {
            return None;
        }
        let slave = libc::open(
            name.as_ptr(),
            libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC,
        );
        if slave < 0 {
            return None;
        }
        let slave_file = File::from_raw_fd(slave);
        let mut tio: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(slave, &mut tio) == 0 {
            libc::cfmakeraw(&mut tio);
            libc::tcsetattr(slave, libc::TCSANOW, &tio);
        }
        let mut ws: libc::winsize = std::mem::zeroed();
        if libc::ioctl(size_from, libc::TIOCGWINSZ, &mut ws) == 0 {
            libc::ioctl(master, libc::TIOCSWINSZ, &ws);
        }
        Some(Pty {
            master: master_file,
            slave: slave_file,
        })
    }
}

fn is_tty(fd: RawFd) -> bool {
    // SAFETY: isatty on a standard descriptor.
    unsafe { libc::isatty(fd) == 1 }
}

fn decode(status: std::process::ExitStatus) -> Exit {
    match (status.code(), status.signal()) {
        (Some(c), _) => Exit::Code(c),
        (None, Some(s)) => Exit::Signal(s),
        (None, None) => Exit::Code(1),
    }
}

/// Which of our streams a tee copies to, and how the scanner reads it.
#[derive(Clone, Copy)]
enum Stream {
    /// stderr: scanned for gh's error messages.
    Err,
    /// stdout of `gh api --include`: scanned for the status line and headers.
    Out,
}

/// Copy `src` to our stream and the scanner until EOF (EIO from a pty master counts as EOF).
fn tee(mut src: File, which: Stream, scanner: Arc<Mutex<Scanner>>, done: mpsc::Sender<()>) {
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        match src.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                // A closed or broken output must not stop the drain.
                match which {
                    Stream::Err => {
                        let mut e = std::io::stderr();
                        let _ = e.write_all(&buf[..n]);
                        let _ = e.flush();
                    }
                    Stream::Out => {
                        let mut o = std::io::stdout();
                        let _ = o.write_all(&buf[..n]);
                        let _ = o.flush();
                    }
                }
                if let Ok(mut s) = scanner.lock() {
                    match which {
                        Stream::Err => s.feed(&buf[..n]),
                        Stream::Out => s.feed_headers(&buf[..n]),
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    let _ = done.send(());
}

/// A child stream gh-paced reads: the child's end goes into the Command, the reader is kept.
enum Capture {
    /// Through a pseudo-terminal: the slave is the child's end.
    Pty(Pty),
    /// Through a pipe made by `Command`.
    Pipe,
}

fn capture_for(fd: RawFd) -> Capture {
    if is_tty(fd) {
        if let Some(p) = open_pty(fd) {
            return Capture::Pty(p);
        }
    }
    Capture::Pipe
}

impl Capture {
    fn child_end(&self) -> Stdio {
        match self {
            Capture::Pty(p) => p
                .slave
                .try_clone()
                .map(Stdio::from)
                .unwrap_or_else(|_| Stdio::piped()),
            Capture::Pipe => Stdio::piped(),
        }
    }
}

/// Wait for the child, enforcing the deadline if there is one. Returns the status and whether
/// the deadline stopped it.
fn wait_with_deadline(
    child: &mut std::process::Child,
    deadline_secs: Option<f64>,
) -> std::io::Result<(std::process::ExitStatus, bool)> {
    let Some(limit) = deadline_secs else {
        return child.wait().map(|s| (s, false));
    };
    let start = Instant::now();
    let term_at = start + Duration::from_secs_f64(limit.max(0.0));
    let mut kill_at: Option<Instant> = None;
    loop {
        if let Some(s) = child.try_wait()? {
            return Ok((s, kill_at.is_some()));
        }
        let now = Instant::now();
        match kill_at {
            None if now >= term_at => {
                if let Ok(pid) = i32::try_from(child.id()) {
                    // SAFETY: signalling our own child, which has not been reaped yet.
                    unsafe {
                        libc::kill(pid, libc::SIGTERM);
                    }
                }
                kill_at = Some(now + Duration::from_secs_f64(KILL_GRACE_SECS));
            }
            Some(k) if now >= k => {
                let _ = child.kill();
                return child.wait().map(|s| (s, true));
            }
            _ => {}
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn base_command(inv: &Invocation<'_>) -> Command {
    let mut cmd = Command::new(inv.program);
    cmd.args(inv.args);
    for var in ["GH_NO_UPDATE_NOTIFIER", "GH_NO_EXTENSION_UPDATE_NOTIFIER"] {
        if std::env::var_os(var).is_none() {
            cmd.env(var, "1");
        }
    }
    for (k, v) in &inv.env {
        cmd.env(k, v);
    }
    cmd
}

fn feed_stdin(child: &mut std::process::Child, data: Option<Vec<u8>>) {
    if let (Some(mut pipe), Some(bytes)) = (child.stdin.take(), data) {
        std::thread::spawn(move || {
            let _ = pipe.write_all(&bytes);
        });
    }
}

impl Runner for RealRunner {
    fn run(&self, inv: Invocation<'_>, scanner: &mut Scanner) -> Result<Ran, String> {
        let shared = Arc::new(Mutex::new(std::mem::take(scanner)));
        let (done_tx, done_rx) = mpsc::channel();
        let err_cap = capture_for(2);
        let out_cap = if inv.scan_stdout {
            Some(capture_for(1))
        } else {
            None
        };
        let keep_fd = inv.keep_fd;
        let set = signal_set();
        // SAFETY: blocking signals for this thread during spawn; restored below.
        let mut old: libc::sigset_t = unsafe { std::mem::zeroed() };
        unsafe {
            libc::pthread_sigmask(libc::SIG_BLOCK, &set, &mut old);
        }
        let installed = install_forwarders();
        // The Command (and its copies of the pty slaves) is dropped at the end of this block, so
        // the readers see EOF once the child and its descendants close the streams.
        let spawn_result = {
            let mut cmd = base_command(&inv);
            // The forwarded signals are blocked while spawning, and a child inherits the signal
            // mask. Give the child gh-paced's original mask back, after first resetting the
            // forwarding handlers so a signal that arrives between fork and exec takes its
            // default action in the child instead of running the parent's handler there.
            let child_mask = old;
            // SAFETY: the closure runs in the forked child before exec and only calls
            // async-signal-safe functions (signal, pthread_sigmask, fcntl) on copied data.
            unsafe {
                cmd.pre_exec(move || {
                    for (i, s) in FORWARDED.into_iter().enumerate() {
                        if installed[i] {
                            libc::signal(s, libc::SIG_DFL);
                        }
                    }
                    libc::pthread_sigmask(libc::SIG_SETMASK, &child_mask, std::ptr::null_mut());
                    if let Some(fd) = keep_fd {
                        // Clear close-on-exec in the child only, so gh inherits the lease.
                        if libc::fcntl(fd, libc::F_SETFD, 0) != 0 {
                            return Err(std::io::Error::last_os_error());
                        }
                    }
                    Ok(())
                });
            }
            cmd.stdin(if inv.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::inherit()
            });
            match &out_cap {
                Some(c) => cmd.stdout(c.child_end()),
                None => cmd.stdout(Stdio::inherit()),
            };
            cmd.stderr(err_cap.child_end());
            cmd.spawn()
        };
        let mut child = match spawn_result {
            Ok(c) => c,
            Err(e) => {
                // SAFETY: restoring the saved mask.
                unsafe {
                    libc::pthread_sigmask(libc::SIG_SETMASK, &old, std::ptr::null_mut());
                }
                return Err(format!("cannot run {}: {e}", inv.program.display()));
            }
        };
        CHILD_PID.store(i32::try_from(child.id()).unwrap_or(0), Ordering::SeqCst);
        // SAFETY: restoring the saved mask; pending signals are now forwarded to the child.
        unsafe {
            libc::pthread_sigmask(libc::SIG_SETMASK, &old, std::ptr::null_mut());
        }
        let err_src: Option<File> = match err_cap {
            Capture::Pipe => child
                .stderr
                .take()
                .map(|e| File::from(std::os::fd::OwnedFd::from(e))),
            Capture::Pty(p) => {
                drop(p.slave);
                Some(p.master)
            }
        };
        let out_src: Option<File> = match out_cap {
            None => None,
            Some(Capture::Pipe) => child
                .stdout
                .take()
                .map(|o| File::from(std::os::fd::OwnedFd::from(o))),
            Some(Capture::Pty(p)) => {
                drop(p.slave);
                Some(p.master)
            }
        };
        let mut readers = 0;
        for (src, which) in [(err_src, Stream::Err), (out_src, Stream::Out)] {
            if let Some(src) = src {
                let s = Arc::clone(&shared);
                let tx = done_tx.clone();
                std::thread::spawn(move || tee(src, which, s, tx));
                readers += 1;
            }
        }
        drop(done_tx);
        feed_stdin(&mut child, inv.stdin);
        let status = wait_with_deadline(&mut child, inv.deadline_secs)
            .map_err(|e| format!("waiting for {}: {e}", inv.program.display()));
        CHILD_PID.store(0, Ordering::SeqCst);
        // A grandchild (pager, credential helper) may keep a stream open after gh exits; give
        // the readers two seconds in total to drain.
        let drain_until = Instant::now() + Duration::from_secs(2);
        for _ in 0..readers {
            let left = drain_until.saturating_duration_since(Instant::now());
            if done_rx.recv_timeout(left).is_err() {
                break;
            }
        }
        if let Ok(s) = shared.lock() {
            *scanner = s.clone();
        }
        status.map(|(s, deadline_hit)| Ran {
            exit: decode(s),
            deadline_hit,
        })
    }

    fn capture(&self, inv: Invocation<'_>, timeout_secs: f64) -> Result<Captured, String> {
        let mut cmd = base_command(&inv);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("cannot run {}: {e}", inv.program.display()))?;
        let mut out = child.stdout.take();
        let mut err = child.stderr.take();
        let out_t = std::thread::spawn(move || {
            let mut v = Vec::new();
            if let Some(o) = out.as_mut() {
                let _ = o.take(1 << 20).read_to_end(&mut v);
            }
            v
        });
        let err_t = std::thread::spawn(move || {
            let mut v = Vec::new();
            if let Some(e) = err.as_mut() {
                let _ = e.take(1 << 20).read_to_end(&mut v);
            }
            v
        });
        let deadline = Instant::now() + Duration::from_secs_f64(timeout_secs.max(0.1));
        let mut timed_out = false;
        let status = loop {
            match child.try_wait() {
                Ok(Some(s)) => break s,
                Ok(None) if Instant::now() >= deadline => {
                    timed_out = true;
                    let _ = child.kill();
                    break child
                        .wait()
                        .map_err(|e| format!("waiting for {}: {e}", inv.program.display()))?;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(25)),
                Err(e) => return Err(format!("waiting for {}: {e}", inv.program.display())),
            }
        };
        let stdout = out_t.join().unwrap_or_default();
        let stderr = err_t.join().unwrap_or_default();
        Ok(Captured {
            exit: decode(status),
            stdout,
            stderr,
            timed_out,
        })
    }
}

/// True when gh-paced's stdin is a terminal.
pub fn stdin_is_tty() -> bool {
    // SAFETY: isatty on a standard descriptor.
    unsafe { libc::isatty(std::io::stdin().as_raw_fd()) == 1 }
}
