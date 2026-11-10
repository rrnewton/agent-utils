//! Per-process /proc reads for the slot-use censuses (#1173
//! census-vanished-process).
//!
//! A process can exit between the census listing /proc and reading one of
//! its entries. Such a read fails with ENOENT once the entry is gone, or
//! ESRCH when the process exits between the lookup and the read. A process
//! that exited uses nothing, so a census takes that read as no evidence
//! and goes on. EACCES and EPERM count the same only once the process has
//! exited (`unreadable_entry_exited`); on a live process they, and every
//! other errno, keep their meaning. The Python census classifies the same
//! way and prints the same diagnostics.

use std::io;
use std::path::{Path, PathBuf};

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

/// How long a census waits for a process in do_exit, whose entry it may
/// not read, to become a zombie or vanish. The other edition uses the same.
pub(crate) const CENSUS_EXIT_WAIT: std::time::Duration = std::time::Duration::from_millis(200);
const CENSUS_EXIT_POLL: std::time::Duration = std::time::Duration::from_millis(10);

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

/// Whether an entry (`path`, failing with `error`) this user may not read
/// belongs to a process that exited. Once do_exit drops a process's
/// mappings, /proc makes its fd directory root-owned, so an exiting
/// process's descriptors read EACCES even to its own user, yet do_exit
/// releases the descriptors only afterwards. So an exited process (gone,
/// or a zombie) counts as gone at once; one still in do_exit (PF_EXITING)
/// is polled for CENSUS_EXIT_WAIT and counts as gone only if it exits
/// meanwhile; any other process does not. A process stuck in do_exit may
/// still hold the descriptors: it stays indeterminate, and the census
/// refuses.
pub(crate) fn unreadable_entry_exited(
    holder: &str,
    base: &Path,
    path: &Path,
    error: &io::Error,
) -> bool {
    // One read decides: a second could find the process reaped meanwhile.
    match stat_state(base) {
        Err(stat_error) => return is_gone(&stat_error),
        Ok(Some((state, _flags))) if matches!(state.as_str(), "Z" | "X") => return true,
        Ok(Some((_state, flags))) if flags & PF_EXITING != 0 => {}
        Ok(_) => return false,
    }
    let deadline = std::time::Instant::now() + CENSUS_EXIT_WAIT;
    while std::time::Instant::now() < deadline {
        std::thread::sleep(CENSUS_EXIT_POLL);
        if has_exited(base) {
            return true;
        }
    }
    diagnose(format!(
        "census: {holder} still exiting after {} ms ({}: {}); indeterminate",
        CENSUS_EXIT_WAIT.as_millis(),
        path.display(),
        errno_name(error)
    ));
    false
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
    static REPLACED: std::cell::RefCell<Vec<(PathBuf, Vec<u8>, bool)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
fn replaced(path: &Path) -> Option<Vec<u8>> {
    let (content, until_exit) = REPLACED.with(|replaced| {
        replaced
            .borrow()
            .iter()
            .find(|(target, _content, _until_exit)| target == path)
            .map(|(_target, content, until_exit)| (content.clone(), *until_exit))
    })?;
    if until_exit {
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

/// Test seam: census reads of `path` on this thread return `content`.
#[cfg(test)]
pub(crate) fn replace(path: &Path, content: &[u8]) {
    REPLACED.with(|replaced| {
        replaced
            .borrow_mut()
            .push((path.to_path_buf(), content.to_vec(), false))
    });
}

/// Test seam: as `replace`, until the real stat at `path` shows the
/// process exited (gone, or a zombie).
#[cfg(test)]
pub(crate) fn replace_until_exit(path: &Path, content: &[u8]) {
    REPLACED.with(|replaced| {
        replaced
            .borrow_mut()
            .push((path.to_path_buf(), content.to_vec(), true))
    });
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
