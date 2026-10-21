//! Raw system calls. The only module allowed to contain `unsafe` code.

/// Real UID of the process.
pub fn uid() -> u32 {
    // SAFETY: getuid takes no argument and cannot fail.
    unsafe { libc::getuid() }
}

/// Clock ticks per second, the unit of CPU times in `/proc/<pid>/stat`.
pub fn clock_ticks() -> u64 {
    // SAFETY: sysconf only reads the requested constant.
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if ticks > 0 {
        ticks as u64
    } else {
        100
    }
}

/// Does the process still exist? (signal 0: check without sending anything)
pub fn pid_alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    // SAFETY: kill with signal 0 has no effect on the target.
    let ret = unsafe { libc::kill(pid, 0) };
    ret == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Exclusive lock (flock) held as long as the file stays open. Blocking.
pub fn lock_exclusive(file: &std::fs::File) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    loop {
        // SAFETY: the descriptor is valid for the whole duration of the call.
        let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
        if ret == 0 {
            return Ok(());
        }
        let err = std::io::Error::last_os_error();
        if err.kind() != std::io::ErrorKind::Interrupted {
            return Err(err);
        }
    }
}

/// The child process gets SIGKILL if distroproof dies, even when distroproof
/// itself is killed by SIGKILL: no orphan VMs.
pub fn kill_with_parent(cmd: &mut tokio::process::Command) {
    // SAFETY: the closure only makes a prctl call, which is allowed between
    // fork and exec (async-signal-safe), without allocating.
    unsafe {
        cmd.pre_exec(|| {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
    }
}

/// The child process, QEMU and its vCPU threads, runs only on `cpus`.
pub fn set_affinity(cmd: &mut tokio::process::Command, cpus: &[usize]) {
    // SAFETY: an all-zero cpu_set_t is the empty set; CPU_SET writes one bit
    // of it and ignores CPUs beyond its size.
    let mut set: libc::cpu_set_t = unsafe { std::mem::zeroed() };
    for &cpu in cpus {
        unsafe { libc::CPU_SET(cpu, &mut set) };
    }
    // SAFETY: the closure only makes a sched_setaffinity call on a set it owns,
    // allowed between fork and exec (async-signal-safe), without allocating.
    unsafe {
        cmd.pre_exec(move || {
            if libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set) == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
    }
}
