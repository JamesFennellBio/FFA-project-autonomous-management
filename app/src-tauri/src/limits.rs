//! RAM and CPU limits: Lite (< 3 GB) / Normal (< 6 GB) and the CPU slider.
//!
//! - Linux: the app runs in its own systemd user scope (`systemd-run --user
//!   --scope`), so the kernel enforces MemoryMax / CPUQuota on the app and its
//!   WebView processes. The pc-daemon user service gets the same limits.
//! - Windows: a Job Object with a memory limit and a hard CPU rate cap; the
//!   WebView2 child processes inherit it.
//! - macOS, Android, iOS: no OS cap is available to an app, so the UI holds
//!   back instead (slower polling, shorter charts, fewer animations: see
//!   app/ui/src/prefs.rs). The OS manages memory on phones anyway.

use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub ram_mb: u64,
    /// Percent of the whole machine, 10..=100.
    pub cpu_pct: u8,
}

impl Limits {
    /// From the UI's saved preferences (app/ui/src/prefs.rs).
    pub fn from_prefs(p: &Value) -> Self {
        let lite = p["perf"] == "lite";
        Self {
            ram_mb: if lite { 3 * 1024 } else { 6 * 1024 },
            cpu_pct: p["cpu_pct"].as_u64().unwrap_or(50).clamp(10, 100) as u8,
        }
    }

    /// systemd's CPUQuota: 100 % = one core.
    #[cfg(any(target_os = "linux", test))]
    pub fn cpu_quota_pct(self, cpus: usize) -> u64 {
        u64::from(self.cpu_pct) * cpus.max(1) as u64
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Report {
    /// "hard": the OS enforces it. "soft": the app holds back by itself.
    pub kind: String,
    pub detail: String,
}

fn soft(detail: impl Into<String>) -> Report {
    Report {
        kind: "soft".into(),
        detail: detail.into(),
    }
}

#[cfg(target_os = "linux")]
fn cpus() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get())
}

/// Environment variable naming our scope after the Linux relaunch.
#[cfg(target_os = "linux")]
pub const SCOPE_ENV: &str = "FARMLAB_SCOPE";

/// Linux, before Tauri starts: re-runs this program inside a systemd user
/// scope with the limits, unless it already is in one (or systemd isn't
/// there). Returns only if it didn't relaunch.
#[cfg(target_os = "linux")]
pub fn relaunch_in_scope(l: Limits) {
    use std::os::unix::process::CommandExt;
    if std::env::var_os(SCOPE_ENV).is_some() || std::env::var_os("FARMLAB_NO_SCOPE").is_some() {
        return;
    }
    // Development runs (`cargo tauri dev`) stay as they are unless asked.
    if cfg!(debug_assertions) && std::env::var_os("FARMLAB_SCOPE_DEV").is_none() {
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let unit = format!("farmlab-app-{}", std::process::id());
    let err = std::process::Command::new("systemd-run")
        .args(["--user", "--scope", "--quiet", "--collect", "--unit", &unit])
        .arg(format!("--property=MemoryMax={}M", l.ram_mb))
        .arg(format!("--property=CPUQuota={}%", l.cpu_quota_pct(cpus())))
        .arg("--")
        .arg(exe)
        .args(std::env::args_os().skip(1))
        .env(SCOPE_ENV, format!("{unit}.scope"))
        .exec();
    // Only reached if systemd-run couldn't start: carry on without the scope.
    eprintln!("farmlab: no systemd scope ({err}); RAM and CPU limits are best effort");
}

#[cfg(not(target_os = "linux"))]
pub fn relaunch_in_scope(_: Limits) {}

#[cfg(target_os = "linux")]
fn set_unit(unit: &str, l: Limits) -> bool {
    std::process::Command::new("systemctl")
        .args(["--user", "set-property", "--runtime", unit])
        .arg(format!("MemoryMax={}M", l.ram_mb))
        .arg(format!("CPUQuota={}%", l.cpu_quota_pct(cpus())))
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Applies `l` now (also when the slider moves).
#[cfg(target_os = "linux")]
pub fn apply(l: Limits) -> Report {
    let gb = l.ram_mb / 1024;
    // The daemon is a systemd user service (deploy/farmlab-daemon.service).
    let daemon = set_unit("farmlab-daemon.service", l);
    let daemon_note = if daemon {
        " The PC daemon has the same limits."
    } else {
        ""
    };
    match std::env::var(SCOPE_ENV) {
        Ok(scope) if set_unit(&scope, l) => Report {
            kind: "hard".into(),
            detail: format!(
                "Memory capped at {gb} GB and processor at {} % by the system.{daemon_note}",
                l.cpu_pct
            ),
        },
        _ => soft(format!(
            "The app slows its refreshes to stay under {} % processor. The system memory cap applies after a restart.{daemon_note}",
            l.cpu_pct
        )),
    }
}

#[cfg(windows)]
mod job {
    use std::sync::OnceLock;

    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_CPU_RATE_CONTROL_ENABLE,
        JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP, JOB_OBJECT_LIMIT_JOB_MEMORY,
        JOBOBJECT_CPU_RATE_CONTROL_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JobObjectCpuRateControlInformation, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    struct Job(HANDLE);
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    /// One job for the app's whole life; WebView2 children inherit it.
    fn job() -> Option<HANDLE> {
        static JOB: OnceLock<Option<Job>> = OnceLock::new();
        JOB.get_or_init(|| unsafe {
            let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if h.is_null() || AssignProcessToJobObject(h, GetCurrentProcess()) == 0 {
                return None;
            }
            Some(Job(h))
        })
        .as_ref()
        .map(|j| j.0)
    }

    pub fn set(ram_mb: u64, cpu_pct: u8) -> bool {
        let Some(h) = job() else { return false };
        unsafe {
            let mut ext: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            ext.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_JOB_MEMORY;
            ext.JobMemoryLimit = (ram_mb * 1024 * 1024) as usize;
            let mem_ok = SetInformationJobObject(
                h,
                JobObjectExtendedLimitInformation,
                (&raw const ext).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ) != 0;
            let mut cpu: JOBOBJECT_CPU_RATE_CONTROL_INFORMATION = std::mem::zeroed();
            cpu.ControlFlags =
                JOB_OBJECT_CPU_RATE_CONTROL_ENABLE | JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP;
            // In 1/100 of a percent of all processors.
            cpu.Anonymous.CpuRate = u32::from(cpu_pct) * 100;
            let cpu_ok = SetInformationJobObject(
                h,
                JobObjectCpuRateControlInformation,
                (&raw const cpu).cast(),
                std::mem::size_of::<JOBOBJECT_CPU_RATE_CONTROL_INFORMATION>() as u32,
            ) != 0;
            mem_ok && cpu_ok
        }
    }
}

#[cfg(windows)]
pub fn apply(l: Limits) -> Report {
    if job::set(l.ram_mb, l.cpu_pct) {
        Report {
            kind: "hard".into(),
            detail: format!(
                "Memory capped at {} GB and processor at {} % by Windows.",
                l.ram_mb / 1024,
                l.cpu_pct
            ),
        }
    } else {
        soft("Windows wouldn't set a limit; the app slows its refreshes instead.")
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
pub fn apply(l: Limits) -> Report {
    soft(format!(
        "This system has no hard cap for apps; the app slows its refreshes to stay near {} % processor.",
        l.cpu_pct
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn limits_from_prefs() {
        let l = Limits::from_prefs(&json!({ "perf": "lite", "cpu_pct": 25 }));
        assert_eq!(
            l,
            Limits {
                ram_mb: 3072,
                cpu_pct: 25
            }
        );
        assert_eq!(l.cpu_quota_pct(8), 200);
        // Missing or silly values fall back to safe ones.
        assert_eq!(
            Limits::from_prefs(&json!({ "cpu_pct": 900 })),
            Limits {
                ram_mb: 6144,
                cpu_pct: 100
            }
        );
        assert_eq!(
            Limits::from_prefs(&json!(null)),
            Limits {
                ram_mb: 6144,
                cpu_pct: 50
            }
        );
    }
}
