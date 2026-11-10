//! The scheduler's procfs CPU-budget fallback keeps the reason a reader refused.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// A private, removed-on-drop evidence directory for one child run.
struct LogDir(PathBuf);
impl LogDir {
    fn new(label: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("proccpu-reason-log-{label}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for LogDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn native_scheduler_retains_constructor_and_sample_failure_reasons() {
    const CASE: &str = "AU_PROCCPU_REASON_CONTROL";
    if let Ok(stage) = std::env::var(CASE) {
        fn pressure() {
            let mut limit = std::mem::MaybeUninit::<libc::rlimit>::uninit();
            // SAFETY: only this isolated test subprocess lowers its own soft FD limit.
            assert_eq!(
                unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, limit.as_mut_ptr()) },
                0
            );
            let mut limit = unsafe { limit.assume_init() };
            limit.rlim_cur = 64;
            assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limit) }, 0);
        }
        let watcher = match stage.as_str() {
            "registration" => {
                pressure();
                None
            }
            "observation" => Some(std::thread::spawn(|| {
                let deadline = Instant::now() + Duration::from_secs(5);
                loop {
                    if crate::proccpu::live_registrations().unwrap() > 0 {
                        pressure();
                        return;
                    }
                    assert!(Instant::now() < deadline, "registration event deadline");
                    std::thread::sleep(Duration::from_millis(1));
                }
            })),
            _ => panic!("unknown isolated reason case"),
        };
        let cfg = crate::io::dag_from_json(r#"{"steps":[{"group":"g","job":"quiet","desc":"known refusal","cmd":"sleep 2.5","cpu_timeout":1,"timeout":30}]}"#).unwrap();
        let result = crate::scheduler::run_dag(&cfg, 1, false, 0);
        assert!(result.ok);
        if let Some(watcher) = watcher {
            watcher.join().unwrap();
        }
        return;
    }
    for stage in ["registration", "observation"] {
        let logs = LogDir::new(stage);
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "proccpu_fallback_tests::native_scheduler_retains_constructor_and_sample_failure_reasons", "--nocapture"])
            .env(CASE, stage).env(crate::attribution::LOG_DIR_ENV, logs.path()).output().unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "{stage}: {stderr} {}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert_eq!(stderr.matches("CANNOT be enforced").count(), 1, "{stderr}");
        assert!(
            stderr.contains(&format!("{stage} failed: descriptor reserve")),
            "{stderr}"
        );
        if stage == "registration" {
            assert!(
                stderr.contains("not retried after waiting starts"),
                "{stderr}"
            );
        }
        println!("native scheduler reason {stage}: {stderr}");
    }
}
