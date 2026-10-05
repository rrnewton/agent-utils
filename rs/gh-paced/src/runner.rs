//! Running the real `gh`: transparent pass-through with stderr teed through the pushback scanner.
//!
//! The child inherits stdin (unless gh-paced had to buffer it for the content guard) and stdout
//! unchanged. Stderr goes through gh-paced so it can be scanned while it streams: through a
//! pseudo-terminal when gh-paced's own stderr is a terminal (so gh still sees a terminal and
//! keeps its colours, spinners and prompts), otherwise through a pipe. User-sent INT, TERM, HUP
//! and QUIT are forwarded to the child; terminal-generated signals already reach it because it
//! shares the foreground process group. A child killed by a signal makes gh-paced die by the
//! same signal after bookkeeping, so callers see the same status as from gh itself.

use crate::pushback::Scanner;
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
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
    /// Run interactively: stdout inherited, stderr streamed to our stderr and fed to `scanner`.
    fn run(&self, inv: Invocation<'_>, scanner: &mut Scanner) -> Result<Exit, String>;
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

fn open_pty() -> Option<Pty> {
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
        if libc::ioctl(2, libc::TIOCGWINSZ, &mut ws) == 0 {
            libc::ioctl(master, libc::TIOCSWINSZ, &ws);
        }
        Some(Pty {
            master: master_file,
            slave: slave_file,
        })
    }
}

fn stderr_is_tty() -> bool {
    // SAFETY: isatty on a standard descriptor.
    unsafe { libc::isatty(2) == 1 }
}

fn decode(status: std::process::ExitStatus) -> Exit {
    match (status.code(), status.signal()) {
        (Some(c), _) => Exit::Code(c),
        (None, Some(s)) => Exit::Signal(s),
        (None, None) => Exit::Code(1),
    }
}

/// Copy `src` to our stderr and the scanner until EOF (EIO from a pty master counts as EOF).
fn tee(mut src: File, scanner: Arc<Mutex<Scanner>>, done: mpsc::Sender<()>) {
    let mut buf = vec![0u8; 16 * 1024];
    let mut stderr = std::io::stderr();
    loop {
        match src.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                // A closed or broken stderr must not stop the drain.
                let _ = stderr.write_all(&buf[..n]);
                let _ = stderr.flush();
                if let Ok(mut s) = scanner.lock() {
                    s.feed(&buf[..n]);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    let _ = done.send(());
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
    fn run(&self, inv: Invocation<'_>, scanner: &mut Scanner) -> Result<Exit, String> {
        let shared = Arc::new(Mutex::new(std::mem::take(scanner)));
        let (done_tx, done_rx) = mpsc::channel();
        let pty = if stderr_is_tty() { open_pty() } else { None };
        let set = signal_set();
        // SAFETY: blocking signals for this thread during spawn; restored below.
        let mut old: libc::sigset_t = unsafe { std::mem::zeroed() };
        unsafe {
            libc::pthread_sigmask(libc::SIG_BLOCK, &set, &mut old);
        }
        let installed = install_forwarders();
        // The Command (and its copy of the pty slave) is dropped at the end of this block, so
        // the reader sees EOF once the child and its descendants close stderr.
        let (spawn_result, piped) = {
            let mut cmd = base_command(&inv);
            // The forwarded signals are blocked while spawning, and a child inherits the signal
            // mask. Give the child gh-paced's original mask back, after first resetting the
            // forwarding handlers so a signal that arrives between fork and exec takes its
            // default action in the child instead of running the parent's handler there.
            let child_mask = old;
            // SAFETY: the closure runs in the forked child before exec and only calls
            // async-signal-safe functions (signal, pthread_sigmask) on copied data.
            unsafe {
                cmd.pre_exec(move || {
                    for (i, s) in FORWARDED.into_iter().enumerate() {
                        if installed[i] {
                            libc::signal(s, libc::SIG_DFL);
                        }
                    }
                    libc::pthread_sigmask(libc::SIG_SETMASK, &child_mask, std::ptr::null_mut());
                    Ok(())
                });
            }
            cmd.stdin(if inv.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::inherit()
            });
            cmd.stdout(Stdio::inherit());
            let slave = pty.as_ref().and_then(|p| p.slave.try_clone().ok());
            let piped = slave.is_none();
            match slave {
                Some(s) => cmd.stderr(Stdio::from(s)),
                None => cmd.stderr(Stdio::piped()),
            };
            (cmd.spawn(), piped)
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
        let source: Option<File> = if piped {
            child
                .stderr
                .take()
                .map(|e| File::from(std::os::fd::OwnedFd::from(e)))
        } else {
            pty.map(|p| {
                drop(p.slave);
                p.master
            })
        };
        let have_reader = source.is_some();
        if let Some(src) = source {
            let s = Arc::clone(&shared);
            std::thread::spawn(move || tee(src, s, done_tx));
        }
        feed_stdin(&mut child, inv.stdin);
        let status = child
            .wait()
            .map_err(|e| format!("waiting for {}: {e}", inv.program.display()));
        CHILD_PID.store(0, Ordering::SeqCst);
        if have_reader {
            // A grandchild (pager, credential helper) may keep stderr open after gh exits.
            let _ = done_rx.recv_timeout(Duration::from_secs(2));
        }
        if let Ok(s) = shared.lock() {
            *scanner = s.clone();
        }
        status.map(decode)
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
