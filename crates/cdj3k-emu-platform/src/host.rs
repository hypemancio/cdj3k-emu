//! Host OS feature detection. One-shot, cached for the life of the process.

use std::sync::OnceLock;

/// macOS major version (15 = Sequoia, 14 = Sonoma, 13 = Ventura, …).
/// Returns 0 if the version can't be determined (non-macOS or `uname` fails).
///
/// Mapping: macOS major = Darwin major − 9 (for macOS 11+).
pub fn macos_major_version() -> u32 {
    static CACHED: OnceLock<u32> = OnceLock::new();
    *CACHED.get_or_init(|| {
        #[cfg(not(target_os = "macos"))]
        {
            return 0;
        }
        #[cfg(target_os = "macos")]
        {
            let mut uts: libc::utsname = unsafe { std::mem::zeroed() };
            if unsafe { libc::uname(&mut uts) } != 0 {
                return 0;
            }
            let release = unsafe { std::ffi::CStr::from_ptr(uts.release.as_ptr()) };
            let darwin_major: u32 = release
                .to_string_lossy()
                .split('.')
                .next()
                .and_then(|t| t.parse().ok())
                .unwrap_or(0);
            if darwin_major >= 20 {
                darwin_major - 9
            } else {
                0
            }
        }
    })
}

/// True when QEMU/HVF can use the in-kernel ARM vGIC
/// (`hv_gic_create`, macOS 15+).
///
/// On older macOS releases the host hypervisor has no GIC primitive, so QEMU
/// must emulate the GIC in userspace - slower per-IRQ cost but functional.
pub fn has_hvf_in_kernel_gic() -> bool {
    macos_major_version() >= 15
}

/// [intel-port] True when this build can use HVF for the aarch64 guest.
///
/// HVF only virtualises the host-native ISA, so an aarch64 guest needs an
/// Apple Silicon host. Compile-time is the exact predicate: the app is always
/// built host-native (bundle.sh passes no `--target`), and an x86_64 binary
/// running under Rosetta 2 has no aarch64 HVF either.
pub fn hvf_supported() -> bool {
    cfg!(all(target_os = "macos", target_arch = "aarch64"))
}

/// [intel-port] True when the guest runs under TCG (full binary translation):
/// either HVF is unsupported on this host (Intel Macs) or TCG was forced via
/// `CDJ3K_EMU_TCG=1` (pre-existing dev escape hatch, semantics preserved).
pub fn tcg_active() -> bool {
    static CACHED: OnceLock<bool> = OnceLock::new();
    *CACHED
        .get_or_init(|| !hvf_supported() || std::env::var_os("CDJ3K_EMU_TCG").is_some())
}

/// [intel-port] Multiplier for host-side timeouts that bound *guest* work
/// (shutdown waits, the UI shutdown watchdog). 1 under HVF. Under TCG the
/// guest runs 20-50x slower, so waits sized for near-native speed would
/// expire and take the SIGKILL path, risking a dirty eMMC qcow2. Default is
/// 50 - the high end of the observed range, deliberately generous: every
/// scaled wait is an early-exit poll, so this only stretches the worst case,
/// and a long worst case beats a corrupted image. Override with
/// `CDJ3K_EMU_TCG_SCALE=<n>`.
pub fn guest_time_scale() -> u32 {
    static CACHED: OnceLock<u32> = OnceLock::new();
    *CACHED.get_or_init(|| {
        if !tcg_active() {
            return 1;
        }
        std::env::var("CDJ3K_EMU_TCG_SCALE")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .filter(|&n| n >= 1)
            .unwrap_or(50)
    })
}
