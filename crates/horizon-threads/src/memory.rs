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

#[cfg(windows)]
pub fn process_memory() -> Option<ProcessMemory> {
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
    (ok != 0).then_some(ProcessMemory {
        working_set_bytes: counters.WorkingSetSize as u64,
        private_bytes: Some(counters.PrivateUsage as u64),
    })
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
    let mut fields = status
        .lines()
        .find(|line| line.starts_with("VmRSS:"))?
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
    }
}
