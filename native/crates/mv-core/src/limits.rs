//! System resource limit for heavy work (TTS, video render, Python workers, local models).
//!
//! One percentage (25–100, saved as `system.resourceLimit` in config.json) caps how much of
//! the PC that work may take, so the mouse and other apps stay responsive:
//! - every heavy child process joins one Windows Job Object with a hard CPU rate cap, a
//!   memory limit and below-normal priority (its own children join automatically);
//! - child processes get thread-count env vars (`OMP_NUM_THREADS`, `MANGAVOX_THREADS`, ...);
//! - in-process pools size themselves with [`threads`] and run below normal priority.
//!
//! Changing the percentage updates the job at once, so running work is throttled too.
use std::sync::atomic::{AtomicU8, Ordering};

use anyhow::Result;
use serde_json::json;

pub const MIN_PERCENT: u8 = 25;
pub const MAX_PERCENT: u8 = 100;
pub const DEFAULT_PERCENT: u8 = 75;
/// config.json key.
pub const CONFIG_KEY: &str = "system.resourceLimit";

/// `BELOW_NORMAL_PRIORITY_CLASS`, for `creation_flags` on Windows spawns.
pub const BELOW_NORMAL_PRIORITY: u32 = 0x0000_4000;

/// 0 until first read from config.
static PERCENT: AtomicU8 = AtomicU8::new(0);

/// Clamps any number to the allowed range.
pub fn clamp(p: i64) -> u8 {
    p.clamp(MIN_PERCENT as i64, MAX_PERCENT as i64) as u8
}

/// The saved percentage (read once, then kept in memory).
pub fn percent() -> u8 {
    match PERCENT.load(Ordering::Relaxed) {
        0 => {
            let p = saved_percent();
            PERCENT.store(p, Ordering::Relaxed);
            p
        }
        p => p,
    }
}

/// Reads the percentage from config.json (default when missing or invalid).
pub fn saved_percent() -> u8 {
    crate::config::Config::open_default().get(CONFIG_KEY).and_then(|v| v.as_i64()).map(clamp).unwrap_or(DEFAULT_PERCENT)
}

/// Applies a new percentage now (running jobs included) and saves it.
pub fn set_percent(p: u8) -> Result<()> {
    let p = apply_percent(p);
    crate::config::Config::open_default().set(CONFIG_KEY, json!(p))
}

/// Applies a percentage for this process only, without saving it.
pub fn apply_percent(p: u8) -> u8 {
    let p = clamp(p as i64);
    PERCENT.store(p, Ordering::Relaxed);
    #[cfg(windows)]
    win::apply(p);
    p
}

/// Logical CPUs on this PC.
pub fn cores() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4)
}

/// Worker threads heavy work may use: the cap's share of the CPUs, at least one.
pub fn threads() -> usize {
    threads_for(cores(), percent())
}

fn threads_for(cores: usize, percent: u8) -> usize {
    ((cores * percent as usize) / 100).max(1)
}

/// Env vars that size the thread pools of child processes (OpenMP, BLAS, PyTorch, our
/// own Python scripts read `MANGAVOX_THREADS` for ONNX Runtime).
pub fn child_env() -> Vec<(&'static str, String)> {
    let n = threads().to_string();
    ["OMP_NUM_THREADS", "MKL_NUM_THREADS", "OPENBLAS_NUM_THREADS", "NUMEXPR_MAX_THREADS", "VECLIB_MAXIMUM_THREADS", "MANGAVOX_THREADS"]
        .into_iter()
        .map(|k| (k, n.clone()))
        .collect()
}

/// Puts a freshly spawned heavy child process under the cap. Returns false when it could
/// not be added (non-Windows, or the process already exited).
pub fn adopt(pid: u32) -> bool {
    #[cfg(windows)]
    {
        win::adopt(pid)
    }
    #[cfg(not(windows))]
    {
        let _ = pid;
        false
    }
}

/// Lowers the calling thread's priority while a cap is set, so the UI thread wins.
pub fn lower_current_thread() {
    #[cfg(windows)]
    if percent() < MAX_PERCENT {
        win::lower_current_thread();
    }
}

/// Total CPU seconds used so far by processes under the cap (live and exited).
pub fn heavy_cpu_seconds() -> f64 {
    #[cfg(windows)]
    {
        win::cpu_seconds()
    }
    #[cfg(not(windows))]
    {
        0.0
    }
}

/// Processes currently under the cap.
pub fn heavy_processes() -> u32 {
    #[cfg(windows)]
    {
        win::active_processes()
    }
    #[cfg(not(windows))]
    {
        0
    }
}

#[cfg(windows)]
mod win {
    use std::ffi::c_void;
    use std::sync::OnceLock;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::*;
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    use windows_sys::Win32::System::Threading::{
        GetCurrentThread, OpenProcess, SetThreadPriority, BELOW_NORMAL_PRIORITY_CLASS, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
        THREAD_PRIORITY_BELOW_NORMAL,
    };

    /// A job object handle. Closing it (app exit) ends every process still in it.
    pub struct Job(HANDLE);
    // SAFETY: a job handle is a kernel object reference, usable from any thread.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    impl Drop for Job {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }

    impl Job {
        pub fn new() -> Option<Job> {
            let h = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            (!h.is_null()).then_some(Job(h))
        }

        /// Hard CPU cap in 1/100 of a percent of all CPUs (10000 = no cap) and a memory limit
        /// in bytes (0 = none). Processes run below normal priority and die with the app.
        pub fn configure(&self, cpu_rate: u32, memory: usize) -> bool {
            let mut ext: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
            ext.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_PRIORITY_CLASS;
            ext.BasicLimitInformation.PriorityClass = BELOW_NORMAL_PRIORITY_CLASS;
            if memory > 0 {
                ext.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_JOB_MEMORY;
                ext.JobMemoryLimit = memory;
            }
            let ok_ext = unsafe { set(self.0, JobObjectExtendedLimitInformation, &ext) };
            let mut cpu: JOBOBJECT_CPU_RATE_CONTROL_INFORMATION = unsafe { std::mem::zeroed() };
            if cpu_rate < 10_000 {
                cpu.ControlFlags = JOB_OBJECT_CPU_RATE_CONTROL_ENABLE | JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP;
                cpu.Anonymous.CpuRate = cpu_rate.max(1);
            }
            let ok_cpu = unsafe { set(self.0, JobObjectCpuRateControlInformation, &cpu) };
            ok_ext && ok_cpu
        }

        pub fn assign(&self, pid: u32) -> bool {
            unsafe {
                let p = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
                if p.is_null() {
                    return false;
                }
                let ok = AssignProcessToJobObject(self.0, p) != 0;
                CloseHandle(p);
                ok
            }
        }

        fn accounting(&self) -> Option<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION> {
            let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
            let ok = unsafe {
                QueryInformationJobObject(
                    self.0,
                    JobObjectBasicAccountingInformation,
                    &mut info as *mut _ as *mut c_void,
                    std::mem::size_of_val(&info) as u32,
                    std::ptr::null_mut(),
                )
            };
            (ok != 0).then_some(info)
        }

        /// User + kernel CPU seconds of every process that has been in the job.
        pub fn cpu_seconds(&self) -> f64 {
            self.accounting().map(|a| (a.TotalUserTime + a.TotalKernelTime) as f64 / 1e7).unwrap_or(0.0)
        }

        pub fn active_processes(&self) -> u32 {
            self.accounting().map(|a| a.ActiveProcesses).unwrap_or(0)
        }
    }

    unsafe fn set<T>(h: HANDLE, class: JOBOBJECTINFOCLASS, info: &T) -> bool {
        SetInformationJobObject(h, class, info as *const T as *const c_void, std::mem::size_of::<T>() as u32) != 0
    }

    pub fn total_memory() -> u64 {
        let mut m: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
        m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        if unsafe { GlobalMemoryStatusEx(&mut m) } != 0 {
            m.ullTotalPhys
        } else {
            0
        }
    }

    /// (cpu rate, memory bytes) for a percentage.
    pub fn limits_for(percent: u8, total_memory: u64) -> (u32, usize) {
        if percent >= super::MAX_PERCENT {
            (10_000, 0)
        } else {
            (percent as u32 * 100, (total_memory / 100 * percent as u64) as usize)
        }
    }

    fn job() -> Option<&'static Job> {
        static JOB: OnceLock<Option<Job>> = OnceLock::new();
        JOB.get_or_init(|| {
            let job = Job::new()?;
            let (cpu, mem) = limits_for(super::percent(), total_memory());
            job.configure(cpu, mem);
            Some(job)
        })
        .as_ref()
    }

    pub fn apply(percent: u8) {
        if let Some(job) = job() {
            let (cpu, mem) = limits_for(percent, total_memory());
            job.configure(cpu, mem);
        }
    }

    pub fn adopt(pid: u32) -> bool {
        job().is_some_and(|j| j.assign(pid))
    }

    pub fn cpu_seconds() -> f64 {
        job().map(Job::cpu_seconds).unwrap_or(0.0)
    }

    pub fn active_processes() -> u32 {
        job().map(Job::active_processes).unwrap_or(0)
    }

    pub fn lower_current_thread() {
        unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamps_and_sizes_threads() {
        assert_eq!(clamp(5), MIN_PERCENT);
        assert_eq!(clamp(250), MAX_PERCENT);
        assert_eq!(clamp(60), 60);
        assert_eq!(threads_for(16, 75), 12);
        assert_eq!(threads_for(2, 25), 1);
        assert_eq!(threads_for(8, 100), 8);
    }

    #[cfg(windows)]
    #[test]
    fn limits_for_percent() {
        assert_eq!(win::limits_for(100, 16 << 30), (10_000, 0));
        let (cpu, mem) = win::limits_for(50, 16 << 30);
        assert_eq!(cpu, 5_000);
        assert_eq!(mem as u64, (16u64 << 30) / 100 * 50);
    }

    /// A busy loop in a capped job gets about the CPU share the cap allows, not a full core.
    #[cfg(windows)]
    #[test]
    fn job_cpu_cap_throttles_a_busy_child() {
        use std::os::windows::process::CommandExt;
        let job = win::Job::new().expect("job");
        // Allow half of one core.
        let rate = (5_000 / cores() as u32).max(1);
        assert!(job.configure(rate, 0));
        let mut child = std::process::Command::new("cmd")
            .args(["/c", "for /L %i in (0,0,1) do @rem"])
            .creation_flags(0x0800_0000)
            .spawn()
            .expect("spawn cmd");
        assert!(job.assign(child.id()));
        std::thread::sleep(std::time::Duration::from_secs(3));
        let used = job.cpu_seconds();
        let _ = child.kill();
        let _ = child.wait();
        // Uncapped it would use ~3 s; the cap allows ~1.5 s (CPU rate is enforced per interval).
        assert!(used < 2.3, "capped child used {used:.2}s of CPU in 3s");
        assert!(used > 0.3, "child barely ran: {used:.2}s");
    }
}
