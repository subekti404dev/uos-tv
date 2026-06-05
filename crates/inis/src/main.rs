//! inis — Tiny Init System untuk UOS TV
//! =====================================
//!
//! inis adalah PID 1 yang minimalis — untuk perangkat embedded.
//! Cross-platform: Linux (target), macOS (development).

use nix::sys::signal::Signal;
use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
use nix::unistd::ForkResult;
use std::os::unix::process::CommandExt;
use std::process::{Command, exit};
use std::sync::atomic::{AtomicBool, Ordering};

static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

// ── Mount helper (cross-platform) ──────────────────

#[cfg(target_os = "linux")]
mod mount_impl {
    use nix::mount::{MsFlags, mount};

    pub type MountFlags = MsFlags;
    pub const MS_NOSUID: MsFlags = MsFlags::MS_NOSUID;
    pub const MS_NOEXEC: MsFlags = MsFlags::MS_NOEXEC;
    pub const MS_NODEV: MsFlags = MsFlags::MS_NODEV;
    pub const MS_RDONLY: MsFlags = MsFlags::MS_RDONLY;
    pub const MS_REMOUNT: MsFlags = MsFlags::MS_REMOUNT;
    pub const MS_EMPTY: MsFlags = MsFlags::empty();

    pub fn mount_fs(
        source: Option<&str>,
        target: &str,
        fstype: Option<&str>,
        flags: MsFlags,
        data: Option<&str>,
    ) -> nix::Result<()> {
        mount(source, target, fstype, flags, data)
    }
}

#[cfg(not(target_os = "linux"))]
mod mount_impl {
    use nix::mount::MntFlags;

    pub type MountFlags = MntFlags;
    pub const MS_NOSUID: MntFlags = MntFlags::MNT_NOSUID;
    pub const MS_NOEXEC: MntFlags = MntFlags::MNT_NOEXEC;
    pub const MS_NODEV: MntFlags = MntFlags::MNT_NODEV;
    pub const MS_RDONLY: MntFlags = MntFlags::MNT_RDONLY;
    pub const MS_REMOUNT: MntFlags = MntFlags::MNT_UPDATE;
    pub const MS_EMPTY: MntFlags = MntFlags::empty();

    pub fn mount_fs(
        source: Option<&str>,
        target: &str,
        _fstype: Option<&str>,
        flags: MntFlags,
        data: Option<&str>,
    ) -> nix::Result<()> {
        // macOS/BSD: mount(source, target, flags, data) — no fstype, source is &P1 not Option
        // Fallback: use libc::mount directly for cross-platform compat
        let src = source.unwrap_or("none");
        let d = data.unwrap_or("");
        let ret = unsafe {
            libc::mount(
                src.as_ptr() as *const i8,
                target.as_ptr() as *const i8,
                flags.bits() as i32,
                d.as_ptr() as *mut libc::c_void,
            )
        };
        nix::errno::Errno::result(ret).map(|_| ())
    }
}

use mount_impl::*;

fn main() {
    eprintln!("[inis] UOS TV Init System v0.1.0 starting...");

    mount_pseudo_fs();
    setup_environment();
    mount_data_partition();
    configure_network();

    let monitord_pid = spawn_monitord();
    eprintln!("[inis] monitord started (PID {monitord_pid})");

    harden_rootfs();

    main_loop();
}

/// Harden rootfs: remount / as read-only, mount tmpfs on /var, /home.
/// /data stays writable for OTA + persistent config.
fn harden_rootfs() {
    eprintln!("[inis] Hardening rootfs...");

    // Mount tmpfs on writable runtime dirs that can't live on RO rootfs
    let writable_dirs = [
        ("/var/log", "mode=0755,size=32M"),
        ("/var/tmp", "mode=1777,size=64M"),
        ("/var/lib", "mode=0755,size=16M"),
        ("/root", "mode=0700,size=8M"),
    ];

    for (dir, opts) in &writable_dirs {
        let _ = std::fs::create_dir_all(dir);
        if mount_fs(
            Some("tmpfs"),
            dir,
            Some("tmpfs"),
            MS_NOSUID | MS_NODEV,
            Some(opts),
        )
        .is_ok()
        {
            eprintln!("[inis]   tmpfs on {dir}");
        }
    }

    // Symlink /etc/resolv.conf to /run (tmpfs, writable)
    let _ = std::fs::create_dir_all("/run/network");
    let _ = std::fs::write("/run/network/resolv.conf", "nameserver 10.0.2.3\n");
    let _ = std::fs::remove_file("/etc/resolv.conf");
    let _ = std::os::unix::fs::symlink("/run/network/resolv.conf", "/etc/resolv.conf");

    // Final remount: read-only root (skip if already RO)
    if let Ok(mounts) = std::fs::read_to_string("/proc/mounts") {
        let already_ro = mounts
            .lines()
            .any(|l| l.starts_with("/dev/vda / ext2") && l.contains("ro,"));
        if !already_ro {
            match mount_fs(None, "/", None, MS_REMOUNT | MS_RDONLY, None) {
                Ok(()) => eprintln!("[inis]   / remounted read-only"),
                Err(e) => eprintln!("[inis]   / remount ro failed: {e}"),
            }
        } else {
            eprintln!("[inis]   / already read-only (Alpine init)");
        }
    }
}

fn configure_network() {
    eprintln!("[inis] Configuring network...");

    // Static IP for QEMU user-mode networking (10.0.2.0/24)
    // On real hardware, dhcpcd/NetworkManager handles this
    let cmds = [
        (&["ip", "link", "set", "lo", "up"][..], "loopback"),
        (&["ip", "link", "set", "eth0", "up"][..], "eth0 up"),
        (
            &["ip", "addr", "add", "10.0.2.15/24", "dev", "eth0"][..],
            "eth0 addr",
        ),
        (
            &["ip", "route", "add", "default", "via", "10.0.2.2"][..],
            "default route",
        ),
    ];

    for (argv, label) in cmds {
        let output = std::process::Command::new(argv[0])
            .args(&argv[1..])
            .output();
        match output {
            Ok(o) if o.status.success() => {
                eprintln!("[inis]   net: {label} OK");
            }
            Ok(o) => {
                let stderr = String::from_utf8_lossy(&o.stderr);
                eprintln!("[inis]   net: {label} WARN: {}", stderr.trim());
            }
            Err(e) => {
                eprintln!("[inis]   net: {label} FAIL: {e}");
            }
        }
    }
}

fn mount_pseudo_fs() {
    eprintln!("[inis] Mounting pseudo-filesystems...");

    mount_dev();

    let m = |src: &str, tgt: &str, fs: &str, flags: MountFlags, data: Option<&str>| {
        let r = mount_fs(Some(src), tgt, Some(fs), flags, data);
        if r.is_ok() {
            eprintln!("[inis]   {tgt} mounted");
        } else {
            eprintln!("[inis]   {tgt} mount failed: {:?}", r.err());
        }
    };

    m(
        "proc",
        "/proc",
        "proc",
        MS_NOSUID | MS_NOEXEC | MS_NODEV,
        None,
    );
    m(
        "sysfs",
        "/sys",
        "sysfs",
        MS_NOSUID | MS_NOEXEC | MS_NODEV,
        None,
    );
    m(
        "tmpfs",
        "/run",
        "tmpfs",
        MS_NOSUID | MS_NODEV,
        Some("mode=0755,size=64M"),
    );
    let _ = std::fs::create_dir_all("/run/uos");
    let _ = std::fs::create_dir_all("/run/uos/locks");
    m(
        "tmpfs",
        "/tmp",
        "tmpfs",
        MS_NOSUID | MS_NODEV,
        Some("mode=1777,size=128M"),
    );
    m(
        "devpts",
        "/dev/pts",
        "devpts",
        MS_NOSUID | MS_NOEXEC,
        Some("mode=0620,gid=5"),
    );
}

fn mount_dev() {
    if std::path::Path::new("/dev/null").exists() {
        return;
    }
    let r = mount_fs(
        Some("devtmpfs"),
        "/dev",
        Some("devtmpfs"),
        MS_NOSUID,
        Some("mode=0755"),
    );
    if r.is_ok() {
        eprintln!("[inis]   /dev mounted");
    } else {
        eprintln!("[inis]   /dev mount failed: {:?}", r.err());
    }
}

fn setup_environment() {
    let hostname = "uos-tv";
    if let Err(e) = nix::unistd::sethostname(hostname) {
        eprintln!("[inis] Failed to set hostname: {e}");
    } else {
        eprintln!("[inis] Hostname set to '{hostname}'");
    }

    unsafe {
        std::env::set_var("PATH", "/usr/bin:/usr/sbin:/bin:/sbin");
        std::env::set_var("HOME", "/root");
        std::env::set_var("SHELL", "/bin/sh");
        std::env::set_var("TERM", "linux");
    }
}

fn mount_data_partition() {
    // /data partition for OTA-safe persistent storage.
    // Strategy:
    //   1. If a real partition exists (mmcblk0p5 / vda5 / sda5), mount it.
    //   2. Otherwise, /data is already on the rootfs (ext2). We use it as-is.
    //   3. Never mount tmpfs over /data — it would wipe preseeded config.

    let _ = std::fs::create_dir_all("/data");

    let devices = ["/dev/mmcblk0p5", "/dev/vda5", "/dev/sda5"];
    let mut mounted = false;

    for dev in &devices {
        if !std::path::Path::new(dev).exists() {
            continue;
        }
        for fs in &["ext4", "f2fs"] {
            if mount_fs(Some(dev), "/data", Some(fs), MS_EMPTY, None).is_ok() {
                eprintln!("[inis]   /data mounted ({dev}, {fs})");
                mounted = true;
                break;
            }
        }
        if mounted {
            break;
        }
    }

    if !mounted {
        eprintln!("[inis]   No data partition — using rootfs /data");
    }

    // Ensure essential directories exist
    let _ = std::fs::create_dir_all("/data/etc");
    let _ = std::fs::create_dir_all("/data/etc/uos");
    let _ = std::fs::create_dir_all("/data/apps");
    let _ = std::fs::create_dir_all("/data/cache");
    let _ = std::fs::create_dir_all("/data/ota");
    let _ = std::fs::create_dir_all("/data/logs");

    // First-boot bootstrap: if /data/etc/uos is empty, copy from rootfs preseed
    if std::fs::read_dir("/data/etc/uos")
        .map(|d| d.count())
        .unwrap_or(0)
        == 0
    {
        if let Ok(entries) = std::fs::read_dir("/usr/share/uos/config") {
            for entry in entries.flatten() {
                let src = entry.path();
                let dst = std::path::Path::new("/data/etc/uos").join(entry.file_name());
                let _ = std::fs::copy(&src, &dst);
                eprintln!("[inis]   config: {} → {}", src.display(), dst.display());
            }
        }
    }

    eprintln!(
        "[inis]   /data OK — apps:{}, cache:{}, ota:{}, logs:{}",
        std::fs::read_dir("/data/apps")
            .map(|d| d.count())
            .unwrap_or(0)
            > 0,
        std::path::Path::new("/data/cache").exists(),
        std::path::Path::new("/data/ota").exists(),
        std::path::Path::new("/data/logs").exists(),
    );
}

fn spawn_monitord() -> nix::unistd::Pid {
    let monitord_path = "/usr/bin/monitord";

    if !std::path::Path::new(monitord_path).exists() {
        eprintln!("[inis] WARNING: {monitord_path} not found!");
        eprintln!("[inis] Starting emergency shell...");
        match unsafe { nix::unistd::fork() } {
            Ok(ForkResult::Parent { child }) => child,
            Ok(ForkResult::Child) => {
                let err = Command::new("/bin/sh").exec();
                eprintln!("[inis] Failed to exec shell: {err}");
                exit(1);
            }
            Err(e) => {
                eprintln!("[inis] Fork failed: {e}");
                exit(1);
            }
        }
    } else {
        match unsafe { nix::unistd::fork() } {
            Ok(ForkResult::Parent { child }) => {
                eprintln!("[inis] Spawned monitord (PID {child})");
                child
            }
            Ok(ForkResult::Child) => {
                let err = Command::new(monitord_path).env("UOS_BOOT", "1").exec();
                eprintln!("[inis] Failed to exec monitord: {err}");
                exit(1);
            }
            Err(e) => {
                eprintln!("[inis] Fork failed: {e}");
                exit(1);
            }
        }
    }
}

fn main_loop() {
    let monitord_path = "/usr/bin/monitord";
    let mut monitord_pid: Option<nix::unistd::Pid> = None;

    eprintln!("[inis] Entering main loop (reaping children)...");

    unsafe {
        for sig in 1..32 {
            let _ = nix::sys::signal::signal(
                Signal::try_from(sig).unwrap_or(Signal::SIGTERM),
                nix::sys::signal::SigHandler::SigIgn,
            );
        }
    }

    let mut restart_count: u32 = 0;
    const MAX_RESTARTS: u32 = 5;

    loop {
        // Restart monitord if it died
        if monitord_pid.is_none() {
            if restart_count >= MAX_RESTARTS {
                eprintln!(
                    "[inis] monitord restarted {MAX_RESTARTS} times — giving up, dropping to shell"
                );
                match unsafe { nix::unistd::fork() } {
                    Ok(ForkResult::Child) => {
                        let _ = Command::new("/bin/sh").exec();
                        std::process::exit(1);
                    }
                    _ => {}
                }
                // Parent: just wait indefinitely
                loop {
                    let _ = waitpid(None, Some(WaitPidFlag::WNOHANG));
                    std::thread::sleep(std::time::Duration::from_secs(5));
                }
            }

            restart_count += 1;
            let delay = std::cmp::min(restart_count * 2, 10) as u64;
            eprintln!("[inis] monitord restart #{restart_count} in {delay}s...");
            std::thread::sleep(std::time::Duration::from_secs(delay));

            monitord_pid = Some(spawn_monitord());
            eprintln!("[inis] monitord restarted (PID {})", monitord_pid.unwrap());
        }

        let current_pid = monitord_pid.unwrap();

        match waitpid(None, Some(WaitPidFlag::WNOHANG)) {
            Ok(WaitStatus::Exited(pid, code)) => {
                eprintln!("[inis] Child {pid} exited with code {code}");
                if pid == current_pid {
                    monitord_pid = None;
                }
            }
            Ok(WaitStatus::Signaled(pid, sig, _)) => {
                eprintln!("[inis] Child {pid} killed by signal {sig:?}");
                if pid == current_pid {
                    monitord_pid = None;
                }
            }
            Ok(_) => std::thread::sleep(std::time::Duration::from_millis(100)),
            Err(nix::Error::ECHILD) => {
                monitord_pid = None;
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
            Err(e) => {
                eprintln!("[inis] waitpid error: {e}");
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        }

        if SHUTDOWN_REQUESTED.load(Ordering::Acquire) {
            do_shutdown();
        }
    }
}

fn do_shutdown() {
    eprintln!("[inis] Shutdown requested...");
    unsafe {
        libc::kill(-1, libc::SIGTERM);
    }
    std::thread::sleep(std::time::Duration::from_secs(2));
    unsafe {
        libc::kill(-1, libc::SIGKILL);
    }
    std::thread::sleep(std::time::Duration::from_secs(1));
    unsafe {
        libc::sync();
    }

    let _ = mount_fs(None, "/data", None, MS_REMOUNT | MS_RDONLY, None);

    eprintln!("[inis] Rebooting...");

    #[cfg(target_os = "linux")]
    {
        use nix::sys::reboot::{RebootMode, reboot};
        let _ = reboot(RebootMode::RB_AUTOBOOT);
    }

    eprintln!("[inis] Reboot failed, halting...");
    loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}
