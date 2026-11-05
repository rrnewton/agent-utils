//! Ending what a stopping service leaves in its systemd unit (#870 agentctl-service-slow-stop).
//!
//! systemd counts a service as stopped only once every process in its cgroup has exited
//! (`KillMode=control-group`, the default), and sends SIGKILL to whatever is left only after its
//! stop timeout, 90 s by default. A process the service started can outlive it: a provider's
//! helper reparented away from the service, or the FUSE daemon that serves a packaged helper's
//! mount, which runs in its own session and can sleep through SIGTERM once its clients are gone.
//! One such process held every stop of a long-running chat bridge for the whole 90 s.
//!
//! So a service, as it exits, ends the rest of its unit itself, FUSE daemons last and cleanly:
//!
//! 1. Every other process except the FUSE daemons gets SIGTERM, and up to the grace to exit.
//! 2. Each FUSE daemon's mount is unmounted (`fusermount -u`), which ends a daemon that SIGTERM
//!    does not, and the daemon gets SIGTERM; up to the grace again.
//! 3. Whatever is left gets SIGKILL, and the mount of a FUSE daemon killed that way is detached
//!    (`fusermount -uz`), so no dead mount ("Transport endpoint is not connected") stays behind.
//!
//! Only a process that systemd started itself as a service's main process does this; one that
//! merely inherited systemd's environment never treats its cgroup as its own.

use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

/// How long processes left in a stopping service's unit get at each step before the next.
pub const SURVIVOR_GRACE: Duration = Duration::from_secs(2);

/// The cgroup of the systemd service this process runs as, or `None` outside one: only a
/// process that systemd started itself as a service's main process (systemd sets
/// `INVOCATION_ID`, and is its parent), in a cgroup that is its own `.service` unit. A process
/// started from a shell that merely inherited `INVOCATION_ID`, such as one in a terminal whose
/// server runs as a service, is never treated as one: its cgroup holds other people's processes.
fn service_cgroup() -> Option<PathBuf> {
    std::env::var_os("INVOCATION_ID")?;
    // SAFETY: getppid takes no arguments and cannot fail.
    let parent = unsafe { libc::getppid() };
    let parent_comm = std::fs::read_to_string(format!("/proc/{parent}/comm")).ok()?;
    if parent_comm.trim() != "systemd" {
        return None;
    }
    let text = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    let path = text
        .lines()
        .find_map(|line| line.strip_prefix("0::"))?
        .trim();
    path.ends_with(".service")
        .then(|| Path::new("/sys/fs/cgroup").join(path.trim_start_matches('/')))
}

/// One other process in a service's cgroup.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Member {
    pid: libc::pid_t,
    comm: String,
    state: char,
    /// The FUSE mounts it names on its command line, when it serves any.
    mounts: Vec<PathBuf>,
}

impl std::fmt::Display for Member {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{} {} {}", self.pid, self.comm, self.state)
    }
}

/// Where a cleanup reads processes and mounts, and how it unmounts: the live system, or a test's
/// stand-ins.
struct System<'a> {
    cgroup: &'a Path,
    proc: &'a Path,
    mountinfo: &'a Path,
    unmount: &'a dyn Fn(&Path, bool) -> Result<(), String>,
}

impl System<'_> {
    /// The processes listed in the cgroup other than this one, each with the FUSE mounts its
    /// command line names.
    fn members(&self) -> Vec<Member> {
        let own = libc::pid_t::try_from(std::process::id()).unwrap_or(0);
        let Ok(text) = std::fs::read_to_string(self.cgroup.join("cgroup.procs")) else {
            return Vec::new();
        };
        let fuse = self.fuse_mounts();
        text.split_whitespace()
            .filter_map(|pid| pid.parse::<libc::pid_t>().ok())
            .filter(|pid| *pid != own)
            .filter_map(|pid| {
                let directory = self.proc.join(pid.to_string());
                let stat = std::fs::read_to_string(directory.join("stat")).ok()?;
                let open = stat.find('(')?;
                let close = stat.rfind(')')?;
                let state = stat[close + 1..].trim_start().chars().next()?;
                let arguments = std::fs::read(directory.join("cmdline")).unwrap_or_default();
                let mounts = arguments
                    .split(|byte| *byte == 0)
                    .filter(|argument| !argument.is_empty())
                    .map(|argument| PathBuf::from(std::ffi::OsStr::from_bytes(argument)))
                    .filter(|argument| fuse.contains(argument))
                    .collect();
                Some(Member {
                    pid,
                    comm: stat[open + 1..close].to_owned(),
                    state,
                    mounts,
                })
            })
            .collect()
    }

    /// The mount points of every FUSE file system in the mount table.
    fn fuse_mounts(&self) -> Vec<PathBuf> {
        let Ok(text) = std::fs::read_to_string(self.mountinfo) else {
            return Vec::new();
        };
        text.lines()
            .filter_map(|line| {
                let (before, after) = line.split_once(" - ")?;
                let fstype = after.split_whitespace().next()?;
                if fstype != "fuse" && !fstype.starts_with("fuse.") {
                    return None;
                }
                Some(PathBuf::from(unescape(before.split_whitespace().nth(4)?)))
            })
            .collect()
    }

    /// Of `members`, those still running (a zombie has exited).
    fn alive(&self, members: &[Member]) -> Vec<Member> {
        let listed = self.members();
        members
            .iter()
            .filter(|member| {
                listed
                    .iter()
                    .any(|now| now.pid == member.pid && now.state != 'Z')
            })
            .cloned()
            .collect()
    }

    /// Wait up to `grace` for `members` to exit; those still running.
    fn wait(&self, members: Vec<Member>, grace: Duration) -> Vec<Member> {
        let deadline = Instant::now() + grace;
        let mut left = self.alive(&members);
        while !left.is_empty() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(50));
            left = self.alive(&left);
        }
        left
    }
}

/// A mount point as `/proc/self/mountinfo` writes it, with `\040` and the like decoded.
fn unescape(field: &str) -> std::ffi::OsString {
    let bytes = field.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\' {
            if let Some(Ok(value)) = field
                .get(index + 1..index + 4)
                .map(|digits| u8::from_str_radix(digits, 8))
            {
                decoded.push(value);
                index += 4;
                continue;
            }
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    std::ffi::OsString::from_vec(decoded)
}

use std::os::unix::ffi::{OsStrExt, OsStringExt};

fn describe(members: &[Member]) -> String {
    if members.is_empty() {
        return "none".to_owned();
    }
    members
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn signal(members: &[Member], signal: libc::c_int) {
    for member in members {
        // SAFETY: kill takes scalar arguments; each PID was read from this service's cgroup.
        unsafe { libc::kill(member.pid, signal) };
    }
}

/// Unmount `mount` with `fusermount3`, else `fusermount`; lazily (`-z`) when `lazy`.
fn fusermount(mount: &Path, lazy: bool) -> Result<(), String> {
    let flags = if lazy { "-uz" } else { "-u" };
    let mut last = String::new();
    for tool in ["fusermount3", "fusermount"] {
        match std::process::Command::new(tool)
            .arg(flags)
            .arg(mount)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .output()
        {
            Ok(output) if output.status.success() => return Ok(()),
            Ok(output) => {
                last = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            }
            Err(error) => last = format!("{tool}: {error}"),
        }
    }
    Err(last)
}

/// Log the other processes in this service's unit at `moment`: a stop that systemd ends with
/// SIGKILL waited for one of them, and this names it. Outside a systemd service, logs nothing.
pub fn log_unit_processes(service: &str, moment: &str) {
    if let Some(cgroup) = service_cgroup() {
        let system = live(&cgroup);
        eprintln!(
            "agentctl: {service}: {moment}; other processes in the unit (pid comm state): {}",
            describe(&system.members())
        );
    }
}

fn live(cgroup: &Path) -> System<'_> {
    System {
        cgroup,
        proc: Path::new("/proc"),
        mountinfo: Path::new("/proc/self/mountinfo"),
        unmount: &fusermount,
    }
}

/// End every other process still in this service's unit, as the module documentation describes,
/// giving each step `grace`. Outside a systemd service, does nothing.
pub fn end_unit_survivors(service: &str, grace: Duration) {
    if let Some(cgroup) = service_cgroup() {
        end_survivors(service, &live(&cgroup), grace);
    }
}

/// [`end_unit_survivors`] on `system`; the processes that needed SIGKILL.
fn end_survivors(service: &str, system: &System<'_>, grace: Duration) -> Vec<Member> {
    let members = system.members();
    if members.is_empty() {
        return Vec::new();
    }
    eprintln!(
        "agentctl: {service}: ending the processes left in the unit: {}",
        describe(&members)
    );
    let (daemons, others): (Vec<Member>, Vec<Member>) = members
        .into_iter()
        .partition(|member| !member.mounts.is_empty());
    signal(&others, libc::SIGTERM);
    let mut left = system.wait(others, grace);
    // The FUSE daemons' clients are gone now, so their mounts are no longer busy: unmounting
    // ends a daemon cleanly even when SIGTERM would not wake it.
    for daemon in &daemons {
        for mount in &daemon.mounts {
            if let Err(error) = (system.unmount)(mount, false) {
                eprintln!(
                    "agentctl: {service}: could not unmount {} for {daemon}: {error}",
                    mount.display()
                );
            }
        }
    }
    signal(&daemons, libc::SIGTERM);
    left.extend(system.wait(daemons, grace));
    if left.is_empty() {
        return left;
    }
    eprintln!(
        "agentctl: {service}: still there after SIGTERM, killed: {}",
        describe(&left)
    );
    signal(&left, libc::SIGKILL);
    // A FUSE daemon killed with its file system still mounted leaves a dead mount; detach it.
    let mounted = system.fuse_mounts();
    for mount in left.iter().flat_map(|member| member.mounts.iter()) {
        if mounted.contains(mount) {
            if let Err(error) = (system.unmount)(mount, true) {
                eprintln!(
                    "agentctl: {service}: could not detach {}: {error}",
                    mount.display()
                );
            }
        }
    }
    left
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;
    use std::process::{Child, Command};

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(label: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("agentctl-unit-{label}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).expect("scratch");
            Self(root)
        }

        fn procs(&self, children: &[&Child]) {
            let mut text = format!("{}\n", std::process::id());
            for child in children {
                text.push_str(&format!("{}\n", child.id()));
            }
            std::fs::write(self.0.join("cgroup.procs"), text).expect("procs");
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn sh(script: &str, arguments: &[&Path]) -> Child {
        let child = Command::new("sh")
            .arg("-c")
            .arg(script)
            .arg("sh")
            .args(arguments)
            .spawn()
            .expect("sh");
        // Let the shell install its traps before any signal comes.
        thread::sleep(Duration::from_millis(300));
        child
    }

    #[test]
    fn processes_left_in_a_unit_get_sigterm_then_sigkill() {
        let scratch = Scratch::new("plain");
        let polite = Command::new("sleep").arg("60").spawn().expect("sleep");
        let stubborn = sh("trap '' TERM; while :; do sleep 0.05; done", &[]);
        scratch.procs(&[&polite, &stubborn]);
        let mountinfo = scratch.0.join("mountinfo");
        std::fs::write(&mountinfo, "").expect("mountinfo");
        let unmounted = std::cell::RefCell::new(Vec::new());
        let unmount = |mount: &Path, lazy: bool| {
            unmounted.borrow_mut().push((mount.to_owned(), lazy));
            Ok(())
        };
        let system = System {
            cgroup: &scratch.0,
            proc: Path::new("/proc"),
            mountinfo: &mountinfo,
            unmount: &unmount,
        };
        let members = system.members();
        assert_eq!(
            members.len(),
            2,
            "this process is never a member: {members:?}"
        );
        let started = Instant::now();
        let killed = end_survivors("test", &system, Duration::from_millis(500));
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(
            killed.iter().map(|member| member.pid).collect::<Vec<_>>(),
            [libc::pid_t::try_from(stubborn.id()).expect("pid")]
        );
        let (mut polite, mut stubborn) = (polite, stubborn);
        assert_eq!(polite.wait().expect("wait").signal(), Some(libc::SIGTERM));
        assert_eq!(stubborn.wait().expect("wait").signal(), Some(libc::SIGKILL));
        assert!(
            unmounted.borrow().is_empty(),
            "no FUSE daemon, nothing unmounted"
        );
        assert!(end_survivors("test", &system, Duration::from_millis(500)).is_empty());
    }

    /// A stand-in FUSE daemon that, like the packaged one, sleeps through SIGTERM and exits once
    /// its mount is gone; and its client, which exits on SIGTERM.
    #[test]
    fn a_fuse_daemon_is_unmounted_after_its_clients_exit_and_leaves_no_mount() {
        let scratch = Scratch::new("fuse");
        let mount = scratch.0.join("mnt");
        std::fs::create_dir_all(&mount).expect("mount point");
        let mountinfo = scratch.0.join("mountinfo");
        std::fs::write(
            &mountinfo,
            format!(
                "1196 1169 0:4699 / {} rw,nosuid shared:3036 - fuse.squashfuse_ll squashfuse_ll rw\n",
                mount.display()
            ),
        )
        .expect("mountinfo");
        let daemon = sh(
            "trap '' TERM; while [ ! -e \"$1/unmounted\" ]; do sleep 0.05; done",
            &[&mount],
        );
        let client = Command::new("sleep").arg("60").spawn().expect("client");
        scratch.procs(&[&client, &daemon]);
        let order = std::cell::RefCell::new(Vec::new());
        let client_pid = client.id();
        let unmount = |target: &Path, lazy: bool| {
            // The client is gone before its daemon's mount is unmounted.
            let client_gone = std::fs::read_to_string(format!("/proc/{client_pid}/stat"))
                .map_or(true, |stat| stat.contains(") Z"));
            order
                .borrow_mut()
                .push((target.to_owned(), lazy, client_gone));
            std::fs::write(target.join("unmounted"), "").map_err(|error| error.to_string())?;
            std::fs::write(&mountinfo, "").map_err(|error| error.to_string())
        };
        let system = System {
            cgroup: &scratch.0,
            proc: Path::new("/proc"),
            mountinfo: &mountinfo,
            unmount: &unmount,
        };
        let members = system.members();
        let found = members
            .iter()
            .find(|member| member.pid == libc::pid_t::try_from(daemon.id()).expect("pid"))
            .expect("daemon is a member");
        assert_eq!(found.mounts, std::slice::from_ref(&mount));
        let killed = end_survivors("test", &system, Duration::from_millis(800));
        assert!(killed.is_empty(), "unmounting ended the daemon: {killed:?}");
        assert_eq!(*order.borrow(), [(mount.clone(), false, true)]);
        let (mut client, mut daemon) = (client, daemon);
        assert_eq!(client.wait().expect("wait").signal(), Some(libc::SIGTERM));
        assert!(daemon.wait().expect("wait").success());
        assert!(system.fuse_mounts().is_empty(), "no mount is left behind");
    }

    #[test]
    fn a_fuse_daemon_that_survives_its_unmount_is_killed_and_its_mount_detached() {
        let scratch = Scratch::new("fuse-stuck");
        let mount = scratch.0.join("mnt with space");
        std::fs::create_dir_all(&mount).expect("mount point");
        let mountinfo = scratch.0.join("mountinfo");
        let escaped = mount.display().to_string().replace(' ', "\\040");
        std::fs::write(
            &mountinfo,
            format!("1196 1169 0:4699 / {escaped} rw - fuse squashfuse_ll rw\n"),
        )
        .expect("mountinfo");
        let daemon = sh("trap '' TERM; while :; do sleep 0.05; done", &[&mount]);
        scratch.procs(&[&daemon]);
        let calls = std::cell::RefCell::new(Vec::new());
        let unmount = |target: &Path, lazy: bool| {
            calls.borrow_mut().push((target.to_owned(), lazy));
            if lazy {
                std::fs::write(&mountinfo, "").map_err(|error| error.to_string())
            } else {
                Err("Device or resource busy".to_owned())
            }
        };
        let system = System {
            cgroup: &scratch.0,
            proc: Path::new("/proc"),
            mountinfo: &mountinfo,
            unmount: &unmount,
        };
        let killed = end_survivors("test", &system, Duration::from_millis(300));
        assert_eq!(killed.len(), 1);
        assert_eq!(
            *calls.borrow(),
            [(mount.clone(), false), (mount.clone(), true)]
        );
        let mut daemon = daemon;
        assert_eq!(daemon.wait().expect("wait").signal(), Some(libc::SIGKILL));
        assert!(
            system.fuse_mounts().is_empty(),
            "the dead mount was detached"
        );
    }
}
