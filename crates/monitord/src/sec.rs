//! Security hardening module.
//!
//! Applies Linux security measures before spawning child processes:
//!   - Capability bounding set restriction
//!   - Securebits (NO_SETUID_FIXUP, NOROOT)
//!   - No new privileges
//!
//! All operations are best-effort — failures are logged but non-fatal.
//! Uses raw capability numbers to avoid libc version divergence across platforms.

use std::collections::HashMap;
use std::sync::LazyLock;

/// Map friendly capability names → Linux capability index.
/// Uses raw numbers for portability (libc on macOS lacks newer caps).
static CAP_NAME_MAP: LazyLock<HashMap<&'static str, i32>> = LazyLock::new(|| {
    let mut m = HashMap::new();
    m.insert("net_admin", 12);
    m.insert("net_raw", 13);
    m.insert("net_bind_service", 10);
    m.insert("sys_admin", 21);
    m.insert("sys_rawio", 17);
    m.insert("sys_ptrace", 19);
    m.insert("sys_nice", 23);
    m.insert("sys_time", 25);
    m.insert("sys_boot", 27);
    m.insert("sys_module", 16);
    m.insert("syslog", 34);
    m.insert("dac_override", 1);
    m.insert("dac_read_search", 2);
    m.insert("fowner", 3);
    m.insert("fsetid", 4);
    m.insert("kill", 5);
    m.insert("setgid", 6);
    m.insert("setuid", 7);
    m.insert("setpcap", 8);
    m.insert("linux_immutable", 9);
    m.insert("net_broadcast", 11);
    m.insert("ipc_lock", 14);
    m.insert("ipc_owner", 15);
    m.insert("sys_tty_config", 26);
    m.insert("sys_resource", 24);
    m.insert("lease", 28);
    m.insert("audit_write", 29);
    m.insert("audit_control", 30);
    m.insert("setfcap", 31);
    m.insert("mac_override", 32);
    m.insert("mac_admin", 33);
    m.insert("sys_chroot", 18);
    m.insert("wake_alarm", 35);
    m.insert("block_suspend", 36);
    m.insert("audit_read", 37);
    m.insert("perfmon", 38);
    m.insert("bpf", 39);
    m.insert("checkpoint_restore", 40);
    m
});

/// Maximum capability index known on this system.
const CAP_LAST: i32 = 40;

pub fn resolve_cap(name: &str) -> Option<i32> {
    CAP_NAME_MAP.get(name).copied()
}

/// Drop all capabilities except those in `keep_caps` from the bounding set.
/// Must be called before `exec()` (in the child after fork, in `pre_exec`).
pub fn apply_capability_bounds(keep_caps: &[String]) {
    #[cfg(target_os = "linux")]
    {
        use std::collections::BTreeSet;

        let keep_indices: BTreeSet<i32> = keep_caps
            .iter()
            .filter_map(|name| resolve_cap(name.as_str()))
            .collect();

        for cap in 0..=CAP_LAST {
            if !keep_indices.contains(&cap) {
                let _ = prctl_drop_cap(cap);
            }
        }

        // Securebits: NO_SETUID_FIXUP | NOROOT | NO_SETUID_FIXUP_LOCKED
        let _ = prctl_set_securebits((1 << 0) | (1 << 1) | (1 << 2));

        // No new privileges
        let _ = prctl_no_new_privs();

        tracing::debug!(
            "sec: dropped capabilities — kept {}, dropped {}",
            keep_caps.join(", "),
            CAP_LAST + 1 - keep_indices.len() as i32
        );
    }
    #[cfg(not(target_os = "linux"))]
    {
        _ = keep_caps;
        tracing::debug!("sec: capability bounds skipped (non-Linux platform)");
    }
}

/// Drop all capabilities (full sandbox).
pub fn drop_all_capabilities() {
    apply_capability_bounds(&[]);
}

#[cfg(target_os = "linux")]
fn prctl_drop_cap(cap: i32) -> nix::Result<()> {
    let rc = unsafe { libc::prctl(libc::PR_CAPBSET_DROP, cap as libc::c_ulong, 0, 0, 0) };
    if rc != 0 {
        let err = nix::errno::Errno::last();
        if err != nix::errno::Errno::EINVAL {
            tracing::warn!("prctl(PR_CAPBSET_DROP, {cap}) failed: {err}");
        }
        return Err(err);
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn prctl_set_securebits(bits: libc::c_ulong) -> nix::Result<()> {
    nix::errno::Errno::result(unsafe { libc::prctl(libc::PR_SET_SECUREBITS, bits, 0, 0, 0) })
        .map(|_| ())
        .map_err(|e| {
            tracing::warn!("prctl(PR_SET_SECUREBITS) failed: {e}");
            e
        })
}

#[cfg(target_os = "linux")]
fn prctl_no_new_privs() -> nix::Result<()> {
    nix::errno::Errno::result(unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) })
        .map(|_| ())
        .map_err(|e| {
            tracing::warn!("prctl(PR_SET_NO_NEW_PRIVS) failed: {e}");
            e
        })
}
