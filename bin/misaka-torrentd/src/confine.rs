//! Confinement (§6.5), applied once, before the engine starts any thread.
//!
//! - **Linux**: Landlock — read and write only the store and the runtime directory; read the
//!   configuration, the adopt roots and what name resolution needs; nothing else. seccomp — deny
//!   `execve`, `execveat`, `ptrace`, `process_vm_readv` / `process_vm_writev`, `mount`, `bpf`,
//!   `perf_event_open`, module loading, `kexec_*`, `keyctl` and `userfaultfd`. The worker's
//!   filter in misakas denies `socket` and `connect`, which the daemon needs, so the daemon
//!   carries its own.
//! - **macOS**: `deploy/launchd/misaka-torrentd.sb`, applied by `sandbox-exec` at launch.
//! - **A server** adds a dedicated account and systemd hardening (`deploy/systemd`).

use std::path::PathBuf;

/// The paths confinement leaves open.
#[derive(Debug, Clone, Default)]
pub struct Paths {
    pub read_write: Vec<PathBuf>,
    pub read_only: Vec<PathBuf>,
}

/// RFC-0002 MP-4: whether this process can reach the clearnet. It sends an empty UDP datagram to
/// 192.0.2.1 and 2001:db8::1, documentation addresses (RFC 5737, RFC 3849) that no host answers.
/// Under systemd's `IPAddressDeny=any` the send fails with EPERM; in a loopback-only network
/// namespace, with ENETUNREACH; under a sandbox profile, with EPERM. A TCP connect cannot tell:
/// systemd's filter drops the SYN, so a blocked and an open connect both time out; a TCP connect
/// that succeeds still proves the network open.
pub fn clearnet_blocked() -> Result<(), String> {
    use std::time::Duration;
    for (bind, to) in [("0.0.0.0:0", "192.0.2.1:9"), ("[::]:0", "[2001:db8::1]:9")] {
        let to: std::net::SocketAddr = to.parse().expect("valid");
        if let Ok(sock) = std::net::UdpSocket::bind(bind)
            && sock.send_to(b"", to).is_ok()
        {
            return Err(format!("a UDP datagram to {to} left the process"));
        }
    }
    let probe: std::net::SocketAddr = "192.0.2.1:9".parse().expect("valid");
    if std::net::TcpStream::connect_timeout(&probe, Duration::from_millis(500)).is_ok() {
        return Err("a clearnet TCP connect succeeded".into());
    }
    Ok(())
}

/// Applies what the platform offers and says what is in force.
pub fn apply(paths: &Paths) -> Result<String, String> {
    imp::apply(paths)
}

#[cfg(target_os = "linux")]
mod imp {
    use std::collections::BTreeMap;

    use landlock::{
        ABI, Access, AccessFs, Ruleset, RulesetAttr, RulesetCreatedAttr, RulesetStatus, path_beneath_rules,
    };
    use seccompiler::{BpfProgram, SeccompAction, SeccompFilter, TargetArch};

    use super::Paths;

    /// Read-only paths name resolution and the C++ runtime may need after start.
    const SYSTEM_READ: &[&str] = &[
        "/etc/resolv.conf",
        "/etc/hosts",
        "/etc/nsswitch.conf",
        "/etc/gai.conf",
        "/etc/host.conf",
        "/etc/localtime",
        "/usr/lib",
        "/lib",
        "/lib64",
        "/usr/lib64",
        "/proc/self",
        "/sys/devices/system/cpu",
    ];

    pub fn apply(paths: &Paths) -> Result<String, String> {
        let landlock = landlock(paths)?;
        seccomp()?;
        Ok(format!("landlock ({landlock}) + seccomp (exec, ptrace, mount, bpf, modules, kexec, keyctl denied)"))
    }

    fn landlock(paths: &Paths) -> Result<&'static str, String> {
        let abi = ABI::V3;
        let read = AccessFs::from_read(abi);
        let all = AccessFs::from_all(abi);
        let existing = |v: &[std::path::PathBuf]| v.iter().filter(|p| p.exists()).cloned().collect::<Vec<_>>();
        let system: Vec<std::path::PathBuf> =
            SYSTEM_READ.iter().map(std::path::PathBuf::from).filter(|p| p.exists()).collect();
        let status = Ruleset::default()
            .handle_access(all)
            .map_err(|e| e.to_string())?
            .create()
            .map_err(|e| e.to_string())?
            .add_rules(path_beneath_rules(existing(&paths.read_write), all))
            .map_err(|e| e.to_string())?
            .add_rules(path_beneath_rules(existing(&paths.read_only), read))
            .map_err(|e| e.to_string())?
            .add_rules(path_beneath_rules(system, read))
            .map_err(|e| e.to_string())?
            .restrict_self()
            .map_err(|e| e.to_string())?;
        Ok(match status.ruleset {
            RulesetStatus::FullyEnforced => "fully enforced",
            RulesetStatus::PartiallyEnforced => "partially enforced by this kernel",
            RulesetStatus::NotEnforced => "not supported by this kernel",
        })
    }

    fn seccomp() -> Result<(), String> {
        let denied: &[libc::c_long] = &[
            libc::SYS_execve,
            libc::SYS_execveat,
            libc::SYS_ptrace,
            libc::SYS_process_vm_readv,
            libc::SYS_process_vm_writev,
            libc::SYS_mount,
            libc::SYS_umount2,
            libc::SYS_pivot_root,
            libc::SYS_bpf,
            libc::SYS_perf_event_open,
            libc::SYS_init_module,
            libc::SYS_finit_module,
            libc::SYS_delete_module,
            libc::SYS_kexec_load,
            libc::SYS_kexec_file_load,
            libc::SYS_keyctl,
            libc::SYS_add_key,
            libc::SYS_request_key,
            libc::SYS_userfaultfd,
        ];
        // `c_long` is `i64` here and `i32` on 32-bit targets.
        #[allow(clippy::useless_conversion)]
        let rules: BTreeMap<i64, Vec<seccompiler::SeccompRule>> =
            denied.iter().map(|&nr| (i64::from(nr), vec![])).collect();
        let arch = TargetArch::try_from(std::env::consts::ARCH).map_err(|e| e.to_string())?;
        let filter = SeccompFilter::new(rules, SeccompAction::Allow, SeccompAction::Errno(libc::EPERM as u32), arch)
            .map_err(|e| e.to_string())?;
        let prog: BpfProgram = filter.try_into().map_err(|e: seccompiler::BackendError| e.to_string())?;
        seccompiler::apply_filter_all_threads(&prog).map_err(|e| e.to_string())
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use super::Paths;

    pub fn apply(_: &Paths) -> Result<String, String> {
        if cfg!(target_os = "macos") {
            // Set by whoever launched this process under sandbox-exec (the desktop app, the
            // LaunchAgent): the profile is applied before exec, so it cannot be seen from here.
            if std::env::var_os("MISAKA_TORRENT_SANDBOX").is_some_and(|v| v == "sandbox-exec") {
                return Ok(
                    "macOS sandbox (misaka-torrentd.sb: store, config and network only; no exec, no fork)".into()
                );
            }
            Ok("none in-process; launch under sandbox-exec with deploy/launchd/misaka-torrentd.sb".into())
        } else {
            Ok("none (Open question 8)".into())
        }
    }
}
