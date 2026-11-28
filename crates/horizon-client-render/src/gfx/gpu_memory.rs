//! What the GPU allocator holds at load boundaries (`memory: gpu ...` lines),
//! beside the renderer's own tally of the textures and buffers it keeps:
//! device memory, to read next to the process counters' host memory.

const MIB: f64 = 1024.0 * 1024.0;
/// Allocations named on the line, largest first.
const LARGEST: usize = 5;

/// Logs `memory: gpu <stage>`: wgpu's allocator report (`unavailable` where
/// the backend gives none) and, when given, the renderer's tally in bytes.
pub fn log(device: &wgpu::Device, stage: &str, tally: Option<u64>) {
    let allocator = device.generate_allocator_report().map_or_else(
        || "allocator report unavailable".to_owned(),
        |report| {
            let allocations: Vec<(&str, u64)> = report
                .allocations
                .iter()
                .map(|a| (a.name.as_str(), a.size))
                .collect();
            describe(
                report.total_allocated_bytes,
                report.total_reserved_bytes,
                report.blocks.len(),
                &allocations,
            )
        },
    );
    let tally = tally.map_or_else(String::new, |bytes| {
        format!(" tally_mb={:.1}", bytes as f64 / MIB)
    });
    log::info!("memory: gpu {stage}; {allocator}{tally}");
}

/// `allocated_mb reserved_mb blocks allocations largest=[name size_mb, ...]`.
fn describe(allocated: u64, reserved: u64, blocks: usize, allocations: &[(&str, u64)]) -> String {
    let mut largest = allocations.to_vec();
    largest.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    let named: Vec<String> = largest
        .iter()
        .take(LARGEST)
        .map(|(name, size)| {
            let name = if name.is_empty() { "unnamed" } else { *name };
            format!("{name} {:.1}", *size as f64 / MIB)
        })
        .collect();
    format!(
        "allocated_mb={:.1} reserved_mb={:.1} blocks={blocks} allocations={} largest=[{}]",
        allocated as f64 / MIB,
        reserved as f64 / MIB,
        allocations.len(),
        named.join(", ")
    )
}

#[cfg(test)]
mod tests {
    /// Reserved beyond allocated is memory the allocator keeps but nothing
    /// uses; the largest allocations are named, an unlabelled one too.
    #[test]
    fn line_shows_reserved_and_the_largest_allocations() {
        const MB: u64 = 1024 * 1024;
        let allocations = [
            ("Block sprites", 240 * MB),
            ("", 3 * MB),
            ("Shared atlas", 64 * MB),
            ("a", MB),
            ("b", MB),
            ("c", MB / 2),
        ];
        assert_eq!(
            super::describe(310 * MB, 512 * MB, 4, &allocations),
            "allocated_mb=310.0 reserved_mb=512.0 blocks=4 allocations=6 \
             largest=[Block sprites 240.0, Shared atlas 64.0, unnamed 3.0, a 1.0, b 1.0]"
        );
        assert_eq!(
            super::describe(0, 0, 0, &[]),
            "allocated_mb=0.0 reserved_mb=0.0 blocks=0 allocations=0 largest=[]"
        );
    }
}
