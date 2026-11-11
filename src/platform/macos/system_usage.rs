//! Fork: the machine's CPU and memory use (`crate::platform::system_usage_sample`), read from the
//! kernel: a few system calls each time, a few microseconds, and no process started.

use std::ffi::CStr;
use std::sync::OnceLock;

use crate::platform::{MemoryPressure, SystemUsageSample};

unsafe extern "C" {
    // libc has it only as deprecated, pointing to the `mach2` crate; one declaration is less
    // than a dependency.
    fn mach_host_self() -> libc::mach_port_t;
}

/// The host port, asked for once: every `mach_host_self` call adds a reference to it.
fn host() -> libc::mach_port_t {
    static HOST: OnceLock<libc::mach_port_t> = OnceLock::new();
    // SAFETY: `mach_host_self` takes nothing and only returns the host port's name.
    *HOST.get_or_init(|| unsafe { mach_host_self() })
}

pub(crate) fn system_usage_sample() -> Option<SystemUsageSample> {
    let cpu_ticks = cpu_ticks()?;
    let page_bytes = sysctl_number(c"hw.pagesize")?;
    Some(SystemUsageSample {
        cpu_ticks,
        memory_used_bytes: memory_used_pages()?.saturating_mul(page_bytes),
        memory_total_bytes: sysctl_number(c"hw.memsize")?,
        // 1, 2 and 4: the levels a `DISPATCH_SOURCE_TYPE_MEMORYPRESSURE` source reports.
        memory_pressure: match sysctl_number(c"kern.memorystatus_vm_pressure_level") {
            Some(1) => Some(MemoryPressure::Normal),
            Some(2) => Some(MemoryPressure::Warning),
            Some(4) => Some(MemoryPressure::Critical),
            _ => None,
        },
    })
}

/// All cores' time since boot in clock ticks, as `[busy, all]`: user, system and nice time, and
/// those with idle time. The kernel's counters are 32 bits and wrap, and so do these.
fn cpu_ticks() -> Option<[u32; 2]> {
    let mut info = libc::host_cpu_load_info {
        cpu_ticks: [0; libc::CPU_STATE_MAX as usize],
    };
    let mut count = libc::HOST_CPU_LOAD_INFO_COUNT;
    // SAFETY: `info` is the `host_cpu_load_info` that `HOST_CPU_LOAD_INFO` fills, and `count`
    // its size in `integer_t`s; the kernel writes no more than `count` of them.
    let result = unsafe {
        libc::host_statistics(
            host(),
            libc::HOST_CPU_LOAD_INFO,
            (&raw mut info).cast(),
            &mut count,
        )
    };
    if result != libc::KERN_SUCCESS {
        return None;
    }
    let tick = |state: libc::c_int| info.cpu_ticks[state as usize];
    let busy = tick(libc::CPU_STATE_USER)
        .wrapping_add(tick(libc::CPU_STATE_SYSTEM))
        .wrapping_add(tick(libc::CPU_STATE_NICE));
    Some([busy, busy.wrapping_add(tick(libc::CPU_STATE_IDLE))])
}

/// What Activity Monitor calls Memory Used, in pages: apps' memory (anonymous pages, less
/// purgeable ones), wired memory and what the compressor holds.
fn memory_used_pages() -> Option<u64> {
    // SAFETY: `vm_statistics64` is plain integers, so all zeros is a valid value.
    let mut stats: libc::vm_statistics64 = unsafe { std::mem::zeroed() };
    let mut count = libc::HOST_VM_INFO64_COUNT;
    // SAFETY: `stats` is the `vm_statistics64` that `HOST_VM_INFO64` fills, and `count` its size
    // in `integer_t`s; the kernel writes no more than `count` of them.
    let result = unsafe {
        libc::host_statistics64(
            host(),
            libc::HOST_VM_INFO64,
            (&raw mut stats).cast(),
            &mut count,
        )
    };
    if result != libc::KERN_SUCCESS {
        return None;
    }
    let apps =
        u64::from(stats.internal_page_count).saturating_sub(u64::from(stats.purgeable_count));
    Some(apps + u64::from(stats.wire_count) + u64::from(stats.compressor_page_count))
}

/// A numeric sysctl, 32 or 64 bits.
fn sysctl_number(name: &CStr) -> Option<u64> {
    let mut value = [0_u8; 8];
    let mut len = value.len();
    // SAFETY: `name` is NUL-terminated, and `value` has room for the `len` bytes it's given.
    let result = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            value.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if result != 0 {
        return None;
    }
    match len {
        4 => Some(u64::from(u32::from_ne_bytes([
            value[0], value[1], value[2], value[3],
        ]))),
        8 => Some(u64::from_ne_bytes(value)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_mac_reports_its_cpu_and_memory() {
        let first = system_usage_sample().expect("a reading");
        assert!(first.memory_total_bytes > 0);
        assert!(
            first.memory_used_bytes > 0 && first.memory_used_bytes <= first.memory_total_bytes,
            "{first:?}"
        );
        assert!(first.memory_pressure.is_some(), "{first:?}");
        // Busy time is part of all the time, and both only count up (modulo wrapping).
        let [busy, all] = first.cpu_ticks;
        assert!(all.wrapping_sub(busy) < u32::MAX / 2, "{first:?}");
        let second = system_usage_sample().expect("another");
        assert!(second.cpu_ticks[1].wrapping_sub(all) < u32::MAX / 2);
    }
}
