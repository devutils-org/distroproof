//! Appels système bruts. Seul module autorisé à contenir de l'`unsafe` :
//! chaque bloc justifie pourquoi il est sûr. Les fonctions exposées sont sûres.

use std::ffi::CStr;
use std::io;
use std::os::fd::{FromRawFd, OwnedFd, RawFd};

fn last_errno() -> i32 {
    io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// Transforme le retour d'un syscall créant un descripteur.
fn fd_result(ret: libc::c_long) -> Result<OwnedFd, i32> {
    if ret < 0 {
        Err(last_errno())
    } else {
        // SAFETY : le noyau vient de créer ce descripteur pour nous, personne
        // d'autre ne le possède ; OwnedFd le fermera une seule fois.
        Ok(unsafe { OwnedFd::from_raw_fd(ret as RawFd) })
    }
}

pub struct Uname {
    pub release: String,
    pub version: String,
    pub machine: String,
}

pub fn uname() -> io::Result<Uname> {
    // SAFETY : utsname ne contient que des tableaux de c_char, l'état tout à
    // zéro est valide.
    let mut u: libc::utsname = unsafe { std::mem::zeroed() };
    // SAFETY : `u` est un pointeur valide vers une utsname modifiable.
    if unsafe { libc::uname(&mut u) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let field = |s: &[libc::c_char]| {
        // SAFETY : le noyau termine chaque champ par un NUL, et la structure
        // était initialisée à zéro, donc un NUL existe dans le tableau.
        unsafe { CStr::from_ptr(s.as_ptr()) }
            .to_string_lossy()
            .into_owned()
    };
    Ok(Uname {
        release: field(&u.release),
        version: field(&u.version),
        machine: field(&u.machine),
    })
}

pub fn page_size() -> usize {
    // SAFETY : sysconf n'a pas de précondition.
    let size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if size > 0 {
        size as usize
    } else {
        4096
    }
}

const BPF_MAP_CREATE: libc::c_long = 0;
const BPF_PROG_LOAD: libc::c_long = 5;
/// Taille de `union bpf_attr` transmise : assez pour les champs utilisés, le
/// reste est à zéro, ce que tous les noyaux acceptent.
const BPF_ATTR_SIZE: usize = 128;

fn put_u32(buf: &mut [u8], off: usize, v: u32) {
    buf[off..off + 4].copy_from_slice(&v.to_ne_bytes());
}

fn put_u64(buf: &mut [u8], off: usize, v: u64) {
    buf[off..off + 8].copy_from_slice(&v.to_ne_bytes());
}

fn bpf(cmd: libc::c_long, attr: &mut [u8; BPF_ATTR_SIZE]) -> Result<OwnedFd, i32> {
    // SAFETY : `attr` est un tampon valide de BPF_ATTR_SIZE octets. Les
    // pointeurs qu'il contient (instructions, licence, log) sont posés par les
    // appelants de ce module et restent valides pendant l'appel.
    let ret = unsafe {
        libc::syscall(
            libc::SYS_bpf,
            cmd,
            attr.as_mut_ptr(),
            BPF_ATTR_SIZE as libc::c_uint,
        )
    };
    fd_result(ret)
}

/// Une instruction eBPF encodée (`struct bpf_insn`).
#[derive(Clone, Copy)]
pub struct BpfInsn([u8; 8]);

impl BpfInsn {
    /// `mov64 r0, imm`
    pub fn mov_r0(imm: i32) -> Self {
        Self::encode(0xb7, imm)
    }

    /// `call helper`
    pub fn call(helper: i32) -> Self {
        Self::encode(0x85, helper)
    }

    /// `exit`
    pub fn exit() -> Self {
        Self::encode(0x95, 0)
    }

    fn encode(code: u8, imm: i32) -> Self {
        let mut b = [0u8; 8];
        b[0] = code;
        b[4..8].copy_from_slice(&imm.to_ne_bytes());
        BpfInsn(b)
    }
}

pub struct ProgLoad<'a> {
    pub prog_type: u32,
    pub expected_attach_type: u32,
    pub prog_flags: u32,
    pub kern_version: u32,
    pub insns: &'a [BpfInsn],
    /// Tampon du log du vérificateur (niveau 1) si fourni.
    pub log: Option<&'a mut [u8]>,
}

/// `BPF_PROG_LOAD`. Le programme est fermé dès que le descripteur est lâché.
pub fn bpf_prog_load(req: ProgLoad<'_>) -> Result<OwnedFd, i32> {
    let license = b"GPL\0";
    let insns: Vec<u8> = req.insns.iter().flat_map(|i| i.0).collect();
    let mut attr = [0u8; BPF_ATTR_SIZE];
    put_u32(&mut attr, 0, req.prog_type);
    put_u32(&mut attr, 4, req.insns.len() as u32);
    put_u64(&mut attr, 8, insns.as_ptr() as u64);
    put_u64(&mut attr, 16, license.as_ptr() as u64);
    if let Some(log) = req.log {
        put_u32(&mut attr, 24, 1);
        put_u32(&mut attr, 28, log.len() as u32);
        put_u64(&mut attr, 32, log.as_mut_ptr() as u64);
    }
    put_u32(&mut attr, 40, req.kern_version);
    put_u32(&mut attr, 44, req.prog_flags);
    put_u32(&mut attr, 68, req.expected_attach_type);
    bpf(BPF_PROG_LOAD, &mut attr)
}

pub struct MapCreate {
    pub map_type: u32,
    pub key_size: u32,
    pub value_size: u32,
    pub max_entries: u32,
    pub map_flags: u32,
}

/// `BPF_MAP_CREATE`. La map est détruite dès que le descripteur est lâché.
pub fn bpf_map_create(req: &MapCreate) -> Result<OwnedFd, i32> {
    let mut attr = [0u8; BPF_ATTR_SIZE];
    put_u32(&mut attr, 0, req.map_type);
    put_u32(&mut attr, 4, req.key_size);
    put_u32(&mut attr, 8, req.value_size);
    put_u32(&mut attr, 12, req.max_entries);
    put_u32(&mut attr, 16, req.map_flags);
    bpf(BPF_MAP_CREATE, &mut attr)
}

pub const PERF_TYPE_HARDWARE: u32 = 0;
pub const PERF_TYPE_SOFTWARE: u32 = 1;
pub const PERF_COUNT_HW_CPU_CYCLES: u64 = 0;
pub const PERF_COUNT_SW_CPU_CLOCK: u64 = 0;
/// PERF_ATTR_SIZE_VER0, compris par tous les noyaux.
const PERF_ATTR_SIZE: usize = 64;
const PERF_FLAG_FD_CLOEXEC: libc::c_ulong = 8;

/// `perf_event_open` sur le processus courant, compteur désactivé : rien
/// n'est mesuré, on teste seulement l'ouverture.
pub fn perf_event_open(event_type: u32, config: u64, exclude_kernel: bool) -> Result<OwnedFd, i32> {
    let mut attr = [0u8; PERF_ATTR_SIZE];
    put_u32(&mut attr, 0, event_type);
    put_u32(&mut attr, 4, PERF_ATTR_SIZE as u32);
    put_u64(&mut attr, 8, config);
    // Champs de bits : disabled (0), exclude_kernel (5), exclude_hv (6).
    let mut flags: u64 = 1;
    if exclude_kernel {
        flags |= (1 << 5) | (1 << 6);
    }
    put_u64(&mut attr, 40, flags);
    // SAFETY : `attr` est une perf_event_attr valide de PERF_ATTR_SIZE octets
    // (taille indiquée dans le champ size) qui survit à l'appel.
    let ret = unsafe {
        libc::syscall(
            libc::SYS_perf_event_open,
            attr.as_ptr(),
            0 as libc::pid_t,
            -1 as libc::c_int,
            -1 as libc::c_int,
            PERF_FLAG_FD_CLOEXEC,
        )
    };
    fd_result(ret)
}
