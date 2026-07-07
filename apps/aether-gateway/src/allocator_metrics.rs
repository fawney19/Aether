use aether_runtime::{MetricKind, MetricSample};

pub(crate) fn gateway_allocator_metric_samples() -> Vec<MetricSample> {
    match allocator_snapshot() {
        Some(snapshot) => snapshot.to_metric_samples(),
        None => unavailable_metric_samples(),
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct AllocatorSnapshot {
    allocated_bytes: u64,
    active_bytes: u64,
    resident_bytes: u64,
    mapped_bytes: u64,
    retained_bytes: u64,
    metadata_bytes: u64,
}

impl AllocatorSnapshot {
    fn to_metric_samples(self) -> Vec<MetricSample> {
        vec![
            gauge(
                "gateway_allocator_observability_available",
                "Whether gateway allocator heap metrics were available for this scrape.",
                1,
            ),
            gauge(
                "gateway_allocator_allocated_bytes",
                "Bytes currently allocated by the gateway allocator.",
                self.allocated_bytes,
            ),
            gauge(
                "gateway_allocator_active_bytes",
                "Bytes in active pages managed by the gateway allocator.",
                self.active_bytes,
            ),
            gauge(
                "gateway_allocator_resident_bytes",
                "Bytes resident in physical memory for the gateway allocator.",
                self.resident_bytes,
            ),
            gauge(
                "gateway_allocator_mapped_bytes",
                "Bytes mapped by the gateway allocator.",
                self.mapped_bytes,
            ),
            gauge(
                "gateway_allocator_retained_bytes",
                "Bytes retained by the gateway allocator for future use.",
                self.retained_bytes,
            ),
            gauge(
                "gateway_allocator_metadata_bytes",
                "Bytes used for allocator metadata.",
                self.metadata_bytes,
            ),
            gauge(
                "gateway_allocator_active_to_allocated_basis_points",
                "Active allocator bytes divided by allocated bytes in basis points.",
                ratio_basis_points(self.active_bytes, self.allocated_bytes),
            ),
            gauge(
                "gateway_allocator_resident_to_allocated_basis_points",
                "Resident allocator bytes divided by allocated bytes in basis points.",
                ratio_basis_points(self.resident_bytes, self.allocated_bytes),
            ),
        ]
    }
}

fn unavailable_metric_samples() -> Vec<MetricSample> {
    vec![
        gauge(
            "gateway_allocator_observability_available",
            "Whether gateway allocator heap metrics were available for this scrape.",
            0,
        ),
        gauge(
            "gateway_allocator_allocated_bytes",
            "Bytes currently allocated by the gateway allocator.",
            0,
        ),
        gauge(
            "gateway_allocator_active_bytes",
            "Bytes in active pages managed by the gateway allocator.",
            0,
        ),
        gauge(
            "gateway_allocator_resident_bytes",
            "Bytes resident in physical memory for the gateway allocator.",
            0,
        ),
        gauge(
            "gateway_allocator_mapped_bytes",
            "Bytes mapped by the gateway allocator.",
            0,
        ),
        gauge(
            "gateway_allocator_retained_bytes",
            "Bytes retained by the gateway allocator for future use.",
            0,
        ),
        gauge(
            "gateway_allocator_metadata_bytes",
            "Bytes used for allocator metadata.",
            0,
        ),
        gauge(
            "gateway_allocator_active_to_allocated_basis_points",
            "Active allocator bytes divided by allocated bytes in basis points.",
            0,
        ),
        gauge(
            "gateway_allocator_resident_to_allocated_basis_points",
            "Resident allocator bytes divided by allocated bytes in basis points.",
            0,
        ),
    ]
}

#[cfg(all(feature = "jemalloc", not(target_env = "msvc")))]
fn allocator_snapshot() -> Option<AllocatorSnapshot> {
    refresh_jemalloc_epoch()?;
    Some(AllocatorSnapshot {
        allocated_bytes: read_jemalloc_stat("stats.allocated\0")?,
        active_bytes: read_jemalloc_stat("stats.active\0")?,
        resident_bytes: read_jemalloc_stat("stats.resident\0")?,
        mapped_bytes: read_jemalloc_stat("stats.mapped\0")?,
        retained_bytes: read_jemalloc_stat("stats.retained\0")?,
        metadata_bytes: read_jemalloc_stat("stats.metadata\0")?,
    })
}

#[cfg(not(all(feature = "jemalloc", not(target_env = "msvc"))))]
fn allocator_snapshot() -> Option<AllocatorSnapshot> {
    None
}

#[cfg(all(feature = "jemalloc", not(target_env = "msvc")))]
fn refresh_jemalloc_epoch() -> Option<()> {
    let mut epoch = 1_u64;
    let result = unsafe {
        tikv_jemalloc_sys::mallctl(
            c"epoch".as_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            (&mut epoch as *mut u64).cast(),
            std::mem::size_of::<u64>(),
        )
    };
    if result == 0 {
        Some(())
    } else {
        None
    }
}

#[cfg(all(feature = "jemalloc", not(target_env = "msvc")))]
fn read_jemalloc_stat(name: &str) -> Option<u64> {
    let mut value = 0_usize;
    let mut size = std::mem::size_of::<usize>();
    let result = unsafe {
        tikv_jemalloc_sys::mallctl(
            name.as_ptr().cast(),
            (&mut value as *mut usize).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if result == 0 {
        Some(u64_from_usize(value))
    } else {
        None
    }
}

fn gauge(name: &'static str, help: &'static str, value: u64) -> MetricSample {
    MetricSample::new(name, help, MetricKind::Gauge, value)
}

fn ratio_basis_points(numerator: u64, denominator: u64) -> u64 {
    if denominator == 0 {
        return 0;
    }
    numerator.saturating_mul(10_000) / denominator
}

fn u64_from_usize(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::{gateway_allocator_metric_samples, ratio_basis_points};

    #[test]
    fn renders_allocator_metric_samples() {
        let samples = gateway_allocator_metric_samples();

        assert!(samples
            .iter()
            .any(|sample| sample.name == "gateway_allocator_observability_available"));
        assert!(samples
            .iter()
            .any(|sample| sample.name == "gateway_allocator_allocated_bytes"));
        assert!(samples
            .iter()
            .any(|sample| sample.name == "gateway_allocator_active_to_allocated_basis_points"));
    }

    #[test]
    fn computes_ratio_basis_points() {
        assert_eq!(ratio_basis_points(150, 100), 15_000);
        assert_eq!(ratio_basis_points(1, 0), 0);
    }
}
