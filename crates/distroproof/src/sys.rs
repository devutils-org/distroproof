//! Appels système bruts. Seul module autorisé à contenir du code `unsafe`.

/// UID réel du processus.
pub fn uid() -> u32 {
    // SAFETY: getuid n'a ni argument ni cas d'erreur.
    unsafe { libc::getuid() }
}

/// Ticks d'horloge par seconde, unité des temps CPU de `/proc/<pid>/stat`.
pub fn clock_ticks() -> u64 {
    // SAFETY: sysconf ne lit que la constante demandée.
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if ticks > 0 {
        ticks as u64
    } else {
        100
    }
}

/// Le processus existe-t-il encore ? (signal 0 : vérification sans envoi)
pub fn pid_alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    // SAFETY: kill avec le signal 0 n'a aucun effet sur la cible.
    let ret = unsafe { libc::kill(pid, 0) };
    ret == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Verrou exclusif (flock) tenu tant que le fichier reste ouvert. Bloquant.
pub fn lock_exclusive(file: &std::fs::File) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    loop {
        // SAFETY: le descripteur est valide pendant toute la durée de l'appel.
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

/// Le processus enfant reçoit SIGKILL si distroproof meurt, même tué par
/// SIGKILL : aucune VM orpheline.
pub fn kill_with_parent(cmd: &mut tokio::process::Command) {
    // SAFETY: la fermeture ne fait qu'un appel prctl, autorisé entre fork et
    // exec (async-signal-safe), sans allocation.
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
