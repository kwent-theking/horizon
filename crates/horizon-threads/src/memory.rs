//! Cheap, process-wide samples at resource-loading boundaries. No child process
//! or heap walk is needed, and a failed query never interrupts loading.

/// Byte counts, rather than allocator estimates or GPU memory. On Windows,
/// private bytes are committed private memory (including nonresident pages).
#[derive(Clone, Copy, Debug)]
pub struct ProcessMemory {
    pub working_set_bytes: u64,
    /// Unavailable on platforms without a cheap equivalent of Windows private bytes.
    pub private_bytes: Option<u64>,
}

/// The most the process has held so far: its working set and its committed
/// private bytes (Windows' peak pagefile usage).
#[derive(Clone, Copy, Debug)]
pub struct PeakMemory {
    pub working_set_bytes: u64,
    pub private_bytes: u64,
}

#[cfg(windows)]
fn counters() -> Option<windows_sys::Win32::System::ProcessStatus::PROCESS_MEMORY_COUNTERS_EX> {
    use windows_sys::Win32::System::{
        ProcessStatus::{
            K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
        },
        Threading::GetCurrentProcess,
    };
    let size = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
    let mut counters = PROCESS_MEMORY_COUNTERS_EX {
        cb: size,
        ..Default::default()
    };
    // SAFETY: the pseudo handle needs no closing. The writable structure lives
    // through the call and its size includes the extended PrivateUsage field.
    let ok = unsafe {
        K32GetProcessMemoryInfo(
            GetCurrentProcess(),
            (&mut counters as *mut PROCESS_MEMORY_COUNTERS_EX).cast::<PROCESS_MEMORY_COUNTERS>(),
            size,
        )
    };
    (ok != 0).then_some(counters)
}

#[cfg(windows)]
pub fn process_memory() -> Option<ProcessMemory> {
    let counters = counters()?;
    Some(ProcessMemory {
        working_set_bytes: counters.WorkingSetSize as u64,
        private_bytes: Some(counters.PrivateUsage as u64),
    })
}

#[cfg(windows)]
pub fn peak_memory() -> Option<PeakMemory> {
    let counters = counters()?;
    Some(PeakMemory {
        working_set_bytes: counters.PeakWorkingSetSize as u64,
        private_bytes: counters.PeakPagefileUsage as u64,
    })
}

#[cfg(not(windows))]
pub fn peak_memory() -> Option<PeakMemory> {
    None
}

/// Physical memory the system can hand out now without paging anything out.
#[cfg(windows)]
pub fn available_memory() -> Option<u64> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut status = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    // SAFETY: the writable structure lives through the call and carries its own size.
    let ok = unsafe { GlobalMemoryStatusEx(&mut status) };
    (ok != 0).then_some(status.ullAvailPhys)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn available_memory() -> Option<u64> {
    kib_field(&std::fs::read_to_string("/proc/meminfo").ok()?, "MemAvailable:")
}

#[cfg(not(any(windows, target_os = "linux", target_os = "android")))]
pub fn available_memory() -> Option<u64> {
    None
}

#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn process_memory() -> Option<ProcessMemory> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    Some(ProcessMemory {
        working_set_bytes: resident_bytes(&status)?,
        // VmSize includes reserved address space and is not private bytes.
        private_bytes: None,
    })
}

#[cfg(any(target_os = "linux", target_os = "android", test))]
fn resident_bytes(status: &str) -> Option<u64> {
    kib_field(status, "VmRSS:")
}

/// A `/proc` line `<key> <n> kB`, in bytes.
#[cfg(any(target_os = "linux", target_os = "android", test))]
fn kib_field(text: &str, key: &str) -> Option<u64> {
    let mut fields = text
        .lines()
        .find(|line| line.starts_with(key))?
        .split_whitespace();
    fields.next()?;
    let kib: u64 = fields.next()?.parse().ok()?;
    if fields.next()? != "kB" {
        return None;
    }
    kib.checked_mul(1024)
}

#[cfg(not(any(windows, target_os = "linux", target_os = "android")))]
pub fn process_memory() -> Option<ProcessMemory> {
    None
}

#[cfg(test)]
mod tests {
    #[test]
    fn resident_units_and_missing_counters() {
        assert_eq!(
            super::resident_bytes("VmSize: 99 kB\nVmRSS:\t123 kB\n"),
            Some(125_952)
        );
        for status in [
            "VmSize: 99 kB",
            "VmRSS: ? kB",
            "VmRSS: 2 MB",
            "VmRSS: 18446744073709551615 kB",
        ] {
            assert_eq!(super::resident_bytes(status), None);
        }
    }

    #[test]
    #[cfg(any(windows, target_os = "linux", target_os = "android"))]
    fn samples_the_current_process() {
        let memory = super::process_memory().expect("current process memory");
        assert!(memory.working_set_bytes > 0);
        #[cfg(windows)]
        assert!(memory.private_bytes.is_some_and(|bytes| bytes > 0));
        assert!(super::available_memory().is_some_and(|bytes| bytes > 0));
    }

    #[test]
    fn available_memory_reads_the_meminfo_line() {
        let meminfo = "MemTotal:       16000000 kB\nMemFree:  100 kB\nMemAvailable:    2048 kB\n";
        assert_eq!(super::kib_field(meminfo, "MemAvailable:"), Some(2_097_152));
        assert_eq!(super::kib_field(meminfo, "SwapFree:"), None);
    }
}
