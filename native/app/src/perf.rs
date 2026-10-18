//! Phase 0 measurements: time to first frame and process memory.
use std::sync::OnceLock;
use std::time::Instant;

static START: OnceLock<Instant> = OnceLock::new();
static FIRST: OnceLock<()> = OnceLock::new();

pub fn mark_start() {
    START.get_or_init(Instant::now);
}

/// Called from the root render; logs once.
pub fn first_frame() {
    FIRST.get_or_init(|| {
        let ms = START.get().map(|s| s.elapsed().as_millis()).unwrap_or(0);
        eprintln!("[perf] first frame after {ms} ms, memory {:.1} MB", memory_mb());
    });
}

#[cfg(windows)]
pub fn memory_mb() -> f64 {
    use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    unsafe {
        let mut c: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
        c.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        if GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) != 0 {
            return c.WorkingSetSize as f64 / 1_048_576.0;
        }
    }
    0.0
}

#[cfg(not(windows))]
pub fn memory_mb() -> f64 {
    0.0
}
