//! Per-process /proc reads for the slot-use censuses (#1173
//! census-vanished-process).
//!
//! A process can exit between the census listing /proc and reading one of
//! its entries. Such a read fails with ENOENT once the entry is gone, or
//! ESRCH when the process exits between the lookup and the read. A process
//! that exited uses nothing, so a census takes that read as no evidence
//! and goes on. EACCES and EPERM count the same only once the process has
//! exited (`unreadable_entry_state`); on a live process they, and every
//! other errno, keep their meaning. The Python census classifies the same
//! way and prints the same diagnostics.

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const ENOENT: i32 = 2;
const ESRCH: i32 = 3;

/// Whether `error` says the process behind a /proc entry exited.
pub(crate) fn is_gone(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(ENOENT | ESRCH))
}

pub(crate) fn read_link(path: &Path) -> io::Result<PathBuf> {
    injected(path)?;
    std::fs::read_link(path)
}

pub(crate) fn read_dir(path: &Path) -> io::Result<std::fs::ReadDir> {
    injected(path)?;
    std::fs::read_dir(path)
}

pub(crate) fn read_to_string(path: &Path) -> io::Result<String> {
    injected(path)?;
    std::fs::read_to_string(path)
}

pub(crate) fn read_bytes(path: &Path) -> io::Result<Vec<u8>> {
    read(path)
}

fn read(path: &Path) -> io::Result<Vec<u8>> {
    injected(path)?;
    #[cfg(test)]
    if let Some(content) = replaced(path) {
        return Ok(content);
    }
    std::fs::read(path)
}

/// The kernel's PF_EXITING task flag: set as do_exit starts, before it
/// releases the task's mappings, descriptors, working directory and root.
const PF_EXITING: u64 = 0x0000_0004;

/// How long one census pass waits, in all, for the processes in do_exit
/// whose entries it may not read to become zombies or vanish (#1173,
/// #1174). Every such process in the pass shares the one deadline
/// (`ExitWait`), so ten exiting compilers cost no more than one. Unmapping
/// a large process's memory, which do_exit does after its entries turn
/// unreadable and before it closes its descriptors, took over 200 ms on a
/// loaded gate host. The other edition uses the same.
pub(crate) const CENSUS_EXIT_WAIT: Duration = Duration::from_secs(2);
/// The first recheck of an exiting process comes this soon, and each
/// later one twice as late, up to CENSUS_EXIT_POLL: an ordinary exit
/// costs a few milliseconds.
const CENSUS_EXIT_FIRST_POLL: Duration = Duration::from_millis(5);
const CENSUS_EXIT_POLL: Duration = Duration::from_millis(50);

/// One census pass's shared wait for processes in do_exit (#1174). The
/// deadline starts with the first exiting process the pass meets and
/// bounds every later one: a process met after it has passed gets one
/// more stat read and no wait.
pub(crate) struct ExitWait {
    deadline: std::cell::Cell<Option<Instant>>,
}

impl ExitWait {
    pub(crate) fn new() -> Self {
        ExitWait {
            deadline: std::cell::Cell::new(None),
        }
    }

    /// Whether the process read at `base` exits before the pass's
    /// deadline; if not, how long it was waited for.
    fn wait_for_exit(&self, base: &Path) -> Result<(), Duration> {
        let started = Instant::now();
        let deadline = self.deadline.get().unwrap_or(started + CENSUS_EXIT_WAIT);
        self.deadline.set(Some(deadline));
        let mut poll = CENSUS_EXIT_FIRST_POLL;
        loop {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            std::thread::sleep(poll.min(deadline - now));
            if has_exited(base) {
                return Ok(());
            }
            poll = (poll * 2).min(CENSUS_EXIT_POLL);
        }
        if has_exited(base) {
            return Ok(());
        }
        Err(started.elapsed())
    }
}

/// The scheduler state and flags in `base`/stat, or None once it is gone
/// or unparseable; the error says whether it is gone.
fn stat_state(base: &Path) -> io::Result<Option<(String, u64)>> {
    let raw = read(&base.join("stat"))?;
    let text = String::from_utf8_lossy(&raw);
    let Some(closing) = text.rfind(") ") else {
        return Ok(None);
    };
    let fields: Vec<&str> = text[closing + 2..].split_whitespace().collect();
    let (Some(state), Some(flags)) = (fields.first(), fields.get(6)) else {
        return Ok(None);
    };
    Ok(flags
        .parse::<u64>()
        .ok()
        .map(|flags| (state.to_string(), flags)))
}

/// Whether the process read at `base` has exited: its stat is gone, or
/// it is a zombie (State Z or X). By the zombie state do_exit has
/// released its descriptors, mappings, working directory and root, so it
/// uses nothing. A zombie leader whose threads still run is followed
/// through those threads, never here.
pub(crate) fn has_exited(base: &Path) -> bool {
    match stat_state(base) {
        Ok(state) => state.is_some_and(|(state, _flags)| matches!(state.as_str(), "Z" | "X")),
        Err(error) => is_gone(&error),
    }
}

/// What a process whose entry (`path`, failing with `error`) this user
/// may not read is doing. Once do_exit drops a process's mappings, /proc
/// makes its fd directory root-owned, so an exiting process's descriptors
/// read EACCES even to its own user, yet do_exit releases the descriptors
/// only afterwards. So an exited process (gone, or a zombie) counts as
/// gone at once; one still in do_exit (PF_EXITING) is rechecked under the
/// pass's shared `wait` and counts as gone only if it exits meanwhile; any
/// other process does not. A process stuck in do_exit may still hold the
/// descriptors: it stays indeterminate, and the census refuses. Its
/// memory is gone for good, so the entry never turns readable again; only
/// its exit ends the wait early.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum UnreadableEntry {
    /// Gone or a zombie, at once or within the pass's wait: it uses nothing.
    Exited,
    /// Still in do_exit when the pass's wait ended, after being waited for
    /// this long: it may hold what it held.
    StillExiting { waited: Duration },
    /// Not exiting: a protected (non-dumpable or setuid) process.
    Running,
}

pub(crate) fn unreadable_entry_state(
    holder: &str,
    base: &Path,
    path: &Path,
    error: &io::Error,
    wait: &ExitWait,
) -> UnreadableEntry {
    // One read decides: a second could find the process reaped meanwhile.
    match stat_state(base) {
        Err(stat_error) if is_gone(&stat_error) => return UnreadableEntry::Exited,
        Err(_) => return UnreadableEntry::Running,
        Ok(Some((state, _flags))) if matches!(state.as_str(), "Z" | "X") => {
            return UnreadableEntry::Exited
        }
        Ok(Some((_state, flags))) if flags & PF_EXITING != 0 => {}
        Ok(_) => return UnreadableEntry::Running,
    }
    let Err(waited) = wait.wait_for_exit(base) else {
        return UnreadableEntry::Exited;
    };
    diagnose(format!(
        "census: {holder} still exiting after {} ms ({}: {}); indeterminate",
        waited.as_millis(),
        path.display(),
        errno_name(error)
    ));
    UnreadableEntry::StillExiting { waited }
}

/// The refusal for an entry (`path`, failing with `error`) of `holder`
/// this user may not read, when `state` is not Exited. For a process
/// given up on in do_exit it names the process, its state and how long it
/// was waited for, so the wait can be measured on a loaded host.
pub(crate) fn unreadable_refusal(
    holder: &str,
    path: &Path,
    error: &io::Error,
    state: &UnreadableEntry,
) -> String {
    let mut message = format!(
        "process use is indeterminate because {} is unreadable: {error}",
        path.display()
    );
    if let UnreadableEntry::StillExiting { waited } = state {
        message.push_str(&format!(
            "; {holder} is still exiting (PF_EXITING, its memory already released) after a {} ms wait (this census waits {} ms in all for exiting processes)",
            waited.as_millis(),
            CENSUS_EXIT_WAIT.as_millis()
        ));
    }
    message
}

/// Whether the process read at `base` runs under this user's real uid
/// (the Uid line of its status). A process that cannot be read is not.
pub(crate) fn owned_by_invoker(base: &Path) -> bool {
    let Ok(status) = read(&base.join("status")) else {
        return false;
    };
    let me = rustix::process::getuid().as_raw();
    String::from_utf8_lossy(&status)
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))
        .and_then(|uids| uids.split_whitespace().next())
        .and_then(|real| real.parse::<u32>().ok())
        == Some(me)
}

fn errno_name(error: &io::Error) -> String {
    match error.raw_os_error() {
        Some(ESRCH) => "ESRCH".to_string(),
        Some(ENOENT) => "ENOENT".to_string(),
        Some(1) => "EPERM".to_string(),
        Some(13) => "EACCES".to_string(),
        other => format!("{other:?}"),
    }
}

/// Print one census diagnostic when `WRKSLOTS_CENSUS_DIAGNOSTICS=1`.
fn diagnose(line: String) {
    #[cfg(test)]
    NOTES.with(|notes| notes.borrow_mut().push(line.clone()));
    if std::env::var_os("WRKSLOTS_CENSUS_DIAGNOSTICS").is_some_and(|value| value == "1") {
        eprintln!("{line}");
    }
}

/// Note in the census diagnostics that `holder` (read at `base`) exited
/// while it was read. `gone` holds the reads that showed nothing because
/// the process exited. ESRCH always means it did; ENOENT alone may only
/// be an absent entry (a kernel thread's executable, a closed
/// descriptor), so without ESRCH the note needs `has_exited` too. The
/// note goes to stderr only when `WRKSLOTS_CENSUS_DIAGNOSTICS=1`, as in
/// the other edition, so ordinary output never changes.
pub(crate) fn note_process_gone(holder: &str, base: &Path, gone: &[(PathBuf, io::Error)]) {
    if gone.is_empty() {
        return;
    }
    let first = match gone
        .iter()
        .find(|(_path, error)| error.raw_os_error() == Some(ESRCH))
    {
        Some(first) => first,
        None if has_exited(base) => &gone[0],
        None => return,
    };
    diagnose(format!(
        "census: {holder} exited during the census ({}: {}); skipped",
        first.0.display(),
        errno_name(&first.1)
    ));
}

#[cfg(test)]
thread_local! {
    static INJECTED: std::cell::RefCell<Vec<(PathBuf, i32)>> =
        const { std::cell::RefCell::new(Vec::new()) };
    static NOTES: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
    static REPLACED: std::cell::RefCell<Vec<(PathBuf, Vec<u8>, Until)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// How long a replaced read lasts.
#[cfg(test)]
#[derive(Clone, Copy)]
enum Until {
    Reset,
    RealExit,
    /// This many more reads.
    Reads(usize),
}

#[cfg(test)]
fn replaced(path: &Path) -> Option<Vec<u8>> {
    let (content, until) = REPLACED.with(|replaced| {
        let mut replaced = replaced.borrow_mut();
        let (_target, content, until) = replaced
            .iter_mut()
            .find(|(target, _content, _until)| target == path)?;
        let current = *until;
        if let Until::Reads(left) = until {
            if *left == 0 {
                return None;
            }
            *left -= 1;
        }
        Some((content.clone(), current))
    })?;
    if let Until::RealExit = until {
        // The real stat wins once it shows the process exited.
        let real = std::fs::read(path).ok()?;
        let text = String::from_utf8_lossy(&real);
        let state = text
            .rfind(") ")
            .and_then(|closing| text[closing + 2..].split_whitespace().next());
        if matches!(state, Some("Z" | "X")) {
            return None;
        }
    }
    Some(content)
}

#[cfg(test)]
fn push_replacement(path: &Path, content: &[u8], until: Until) {
    REPLACED.with(|replaced| {
        replaced
            .borrow_mut()
            .push((path.to_path_buf(), content.to_vec(), until))
    });
}

/// Test seam: census reads of `path` on this thread return `content`.
#[cfg(test)]
pub(crate) fn replace(path: &Path, content: &[u8]) {
    push_replacement(path, content, Until::Reset);
}

/// Test seam: as `replace`, until the real stat at `path` shows the
/// process exited (gone, or a zombie).
#[cfg(test)]
pub(crate) fn replace_until_exit(path: &Path, content: &[u8]) {
    push_replacement(path, content, Until::RealExit);
}

/// Test seam: as `replace`, for the next `reads` reads only; then the
/// real file is read.
#[cfg(test)]
pub(crate) fn replace_for(path: &Path, content: &[u8], reads: usize) {
    push_replacement(path, content, Until::Reads(reads));
}

#[cfg(test)]
fn injected(path: &Path) -> io::Result<()> {
    INJECTED.with(|injected| {
        match injected
            .borrow()
            .iter()
            .find(|(target, _errno)| target == path)
        {
            Some((_target, errno)) => Err(io::Error::from_raw_os_error(*errno)),
            None => Ok(()),
        }
    })
}

#[cfg(not(test))]
#[inline]
fn injected(_path: &Path) -> io::Result<()> {
    Ok(())
}

/// Test seam: fail every census read of `path` on this thread with `errno`.
#[cfg(test)]
pub(crate) fn inject(path: &Path, errno: i32) {
    INJECTED.with(|injected| injected.borrow_mut().push((path.to_path_buf(), errno)));
}

/// Clear this thread's injected errors and return the diagnostics noted.
#[cfg(test)]
pub(crate) fn reset() -> Vec<String> {
    INJECTED.with(|injected| injected.borrow_mut().clear());
    REPLACED.with(|replaced| replaced.borrow_mut().clear());
    NOTES.with(|notes| std::mem::take(&mut *notes.borrow_mut()))
}

/// Children that use a scratch slot in one way each, for the census tests.
#[cfg(test)]
pub(crate) mod test_support {
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};

    pub(crate) struct Slot {
        pub(crate) base: PathBuf,
        pub(crate) slot: PathBuf,
        pub(crate) outside: PathBuf,
    }

    impl Slot {
        pub(crate) fn new(label: &str) -> Slot {
            let base = std::env::temp_dir().join(format!(
                "wrkslots-gone-{label}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&base);
            std::fs::create_dir_all(base.join("slot")).unwrap();
            std::fs::create_dir_all(base.join("outside")).unwrap();
            let slot = base.join("slot").canonicalize().unwrap();
            let outside = base.join("outside").canonicalize().unwrap();
            Slot {
                base,
                slot,
                outside,
            }
        }
    }

    impl Drop for Slot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.base);
        }
    }

    /// A child whose working directory is in the slot.
    pub(crate) fn in_cwd(slot: &Slot) -> Child {
        let child = Command::new("sleep")
            .arg("600")
            .current_dir(&slot.slot)
            .stdin(Stdio::null())
            .spawn()
            .unwrap();
        settle(&child, "cwd", &slot.slot);
        child
    }

    /// A child, outside the slot, holding descriptor 3 on a slot file.
    pub(crate) fn holding_fd(slot: &Slot) -> Child {
        let file = slot.slot.join("held");
        std::fs::write(&file, b"x").unwrap();
        let child = Command::new("sh")
            .arg("-c")
            .arg("exec sleep 600 3<\"$0\"")
            .arg(&file)
            .current_dir(&slot.outside)
            .stdin(Stdio::null())
            .spawn()
            .unwrap();
        settle(&child, "fd/3", &file);
        child
    }

    /// A child, outside the slot, running a program copied into it, so
    /// its executable and a mapping are in the slot.
    pub(crate) fn running_from(slot: &Slot) -> Child {
        let program = slot.slot.join("sleep");
        // Copied by another process: a write descriptor open here could be
        // inherited by a child another test forks, and exec would then
        // fail with ETXTBSY.
        let copied = Command::new("cp")
            .arg("/bin/sleep")
            .arg(&program)
            .status()
            .unwrap();
        assert!(copied.success());
        let child = Command::new(&program)
            .arg("600")
            .current_dir(&slot.outside)
            .stdin(Stdio::null())
            .spawn()
            .unwrap();
        settle(&child, "exe", &program);
        child
    }

    /// A child, outside the slot, whose only use of it is a libc mapping
    /// of a slot file whose descriptor it closed; it exits after
    /// `seconds`. Returns once it printed "ready".
    pub(crate) fn mapping_only(slot: &Slot, seconds: &str) -> Child {
        use std::io::BufRead as _;
        let held = slot.slot.join("mapped");
        std::fs::write(&held, vec![b'x'; 4096]).unwrap();
        let script = "import ctypes, os, sys, time\n\
libc = ctypes.CDLL(None, use_errno=True)\n\
libc.mmap.restype = ctypes.c_void_p\n\
libc.mmap.argtypes = (ctypes.c_void_p, ctypes.c_size_t, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_long)\n\
fd = os.open(sys.argv[1], os.O_RDONLY)\n\
assert libc.mmap(None, 4096, 1, 2, fd, 0) not in (None, ctypes.c_void_p(-1).value)\n\
os.close(fd)\n\
print('ready', flush=True)\n\
time.sleep(float(sys.argv[2]))\n";
        // The interpreter itself: a `python3` on PATH may be a launcher
        // that forks it, and the census must meet the mapping PID.
        let interpreter = Command::new("python3")
            .arg("-c")
            .arg("import sys; print(sys.executable)")
            .output()
            .unwrap();
        let interpreter = String::from_utf8(interpreter.stdout).unwrap();
        let mut child = Command::new(interpreter.trim())
            .arg("-c")
            .arg(script)
            .arg(&held)
            .arg(seconds)
            .current_dir(&slot.outside)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        std::io::BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        assert_eq!(line.trim(), "ready");
        child
    }

    fn settle(child: &Child, entry: &str, target: &Path) {
        let link = proc_path(child, entry);
        for _ in 0..500 {
            if std::fs::read_link(&link).is_ok_and(|found| found == target) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("{} never pointed at {}", link.display(), target.display());
    }

    pub(crate) fn proc_path(child: &Child, entry: &str) -> PathBuf {
        PathBuf::from(format!("/proc/{}/{entry}", child.id()))
    }

    pub(crate) fn stop(mut child: Child) {
        let _ = child.kill();
        let _ = child.wait();
    }
}

#[cfg(test)]
mod exit_wait_tests {
    use super::*;

    const EACCES: i32 = 13;

    /// A stat line for a fake process stuck in do_exit (State D, PF_EXITING).
    fn exiting_stat(pid: u32) -> Vec<u8> {
        format!(
            "{pid} (cc1plus) D 1 1 1 0 -1 {} 0 0 0 0 0 0 0 0 20 0 1 0 100 0 0\n",
            PF_EXITING | 0x40
        )
        .into_bytes()
    }

    #[test]
    fn the_wait_ends_as_soon_as_the_process_exits() {
        // A fake /proc/PID: in do_exit for the deciding read and the first
        // recheck, then gone (its real stat does not exist).
        let base = PathBuf::from("/nonexistent-proc/4242");
        let _ = reset();
        replace_for(&base.join("stat"), &exiting_stat(4242), 2);
        let wait = ExitWait::new();
        let error = io::Error::from_raw_os_error(EACCES);
        let started = Instant::now();
        let state = unreadable_entry_state("process 4242", &base, &base.join("fd"), &error, &wait);
        let elapsed = started.elapsed();
        assert_eq!(reset(), Vec::<String>::new());
        assert_eq!(state, UnreadableEntry::Exited);
        // Two short rechecks (5 and 10 ms), nowhere near the whole wait.
        assert!(elapsed < CENSUS_EXIT_WAIT / 4, "{elapsed:?}");
    }

    #[test]
    fn exiting_processes_share_one_deadline() {
        let bases: Vec<PathBuf> = (0..10)
            .map(|index| PathBuf::from(format!("/nonexistent-proc/{}", 5000 + index)))
            .collect();
        let _ = reset();
        for (index, base) in bases.iter().enumerate() {
            replace(&base.join("stat"), &exiting_stat(5000 + index as u32));
        }
        let wait = ExitWait::new();
        let error = io::Error::from_raw_os_error(EACCES);
        let started = Instant::now();
        let states: Vec<UnreadableEntry> = bases
            .iter()
            .enumerate()
            .map(|(index, base)| {
                unreadable_entry_state(
                    &format!("process {}", 5000 + index),
                    base,
                    &base.join("fd"),
                    &error,
                    &wait,
                )
            })
            .collect();
        let elapsed = started.elapsed();
        let notes = reset();
        // One wait in all, not ten.
        assert!(elapsed >= CENSUS_EXIT_WAIT, "{elapsed:?}");
        assert!(elapsed < CENSUS_EXIT_WAIT * 2, "{elapsed:?}");
        assert_eq!(notes.len(), 10, "{notes:?}");
        let UnreadableEntry::StillExiting { waited } = states[0] else {
            panic!("{:?}", states[0]);
        };
        assert!(waited >= CENSUS_EXIT_WAIT, "{waited:?}");
        for state in &states[1..] {
            let UnreadableEntry::StillExiting { waited } = state else {
                panic!("{state:?}");
            };
            assert!(*waited < CENSUS_EXIT_WAIT / 4, "{waited:?}");
        }
        let refusal = unreadable_refusal(
            "process 5003",
            &bases[3].join("fd"),
            &error,
            &UnreadableEntry::StillExiting {
                waited: Duration::from_millis(0),
            },
        );
        assert_eq!(
            refusal,
            format!(
                "process use is indeterminate because /nonexistent-proc/5003/fd is unreadable: {error}; process 5003 is still exiting (PF_EXITING, its memory already released) after a 0 ms wait (this census waits 2000 ms in all for exiting processes)"
            )
        );
    }
}
