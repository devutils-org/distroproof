//! Raw system calls. The only module allowed to contain `unsafe`: each block
//! explains why it is sound. The exposed functions are safe.

use std::ffi::CStr;
use std::io;
use std::os::fd::{FromRawFd, OwnedFd, RawFd};

fn last_errno() -> i32 {
    io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// Turns the return value of a syscall that creates a descriptor into a result.
fn fd_result(ret: libc::c_long) -> Result<OwnedFd, i32> {
    if ret < 0 {
        Err(last_errno())
    } else {
        // SAFETY: the kernel just created this descriptor for us, nobody else
        // owns it; OwnedFd will close it exactly once.
        Ok(unsafe { OwnedFd::from_raw_fd(ret as RawFd) })
    }
}

pub struct Uname {
    pub release: String,
    pub version: String,
    pub machine: String,
}

pub fn uname() -> io::Result<Uname> {
    // SAFETY: utsname only contains c_char arrays, so the all-zero state is
    // valid.
    let mut u: libc::utsname = unsafe { std::mem::zeroed() };
    // SAFETY: `u` is a valid pointer to a mutable utsname.
    if unsafe { libc::uname(&mut u) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let field = |s: &[libc::c_char]| {
        // SAFETY: the kernel NUL-terminates every field, and the struct was
        // zero-initialized, so the array contains a NUL.
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
    // SAFETY: sysconf has no precondition.
    let size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if size > 0 {
        size as usize
    } else {
        4096
    }
}

const BPF_MAP_CREATE: libc::c_long = 0;
const BPF_PROG_LOAD: libc::c_long = 5;
/// Size of the `union bpf_attr` passed in: enough for the fields we use; the
/// rest is zero, which every kernel accepts.
const BPF_ATTR_SIZE: usize = 128;

fn put_u32(buf: &mut [u8], off: usize, v: u32) {
    buf[off..off + 4].copy_from_slice(&v.to_ne_bytes());
}

fn put_u64(buf: &mut [u8], off: usize, v: u64) {
    buf[off..off + 8].copy_from_slice(&v.to_ne_bytes());
}

fn bpf(cmd: libc::c_long, attr: &mut [u8; BPF_ATTR_SIZE]) -> Result<OwnedFd, i32> {
    // SAFETY: `attr` is a valid buffer of BPF_ATTR_SIZE bytes. The pointers it
    // holds (instructions, license, log) are set by this module's callers and
    // stay valid for the duration of the call.
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

/// An encoded eBPF instruction (`struct bpf_insn`).
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
    /// Verifier log buffer (level 1), if provided.
    pub log: Option<&'a mut [u8]>,
}

/// `BPF_PROG_LOAD`. The program is closed as soon as the descriptor is dropped.
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

/// `BPF_MAP_CREATE`. The map is destroyed as soon as the descriptor is dropped.
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
/// PERF_ATTR_SIZE_VER0, understood by every kernel.
const PERF_ATTR_SIZE: usize = 64;
const PERF_FLAG_FD_CLOEXEC: libc::c_ulong = 8;

/// `perf_event_open` on the current process with the counter disabled:
/// nothing is measured, only opening is tested.
pub fn perf_event_open(event_type: u32, config: u64, exclude_kernel: bool) -> Result<OwnedFd, i32> {
    let mut attr = [0u8; PERF_ATTR_SIZE];
    put_u32(&mut attr, 0, event_type);
    put_u32(&mut attr, 4, PERF_ATTR_SIZE as u32);
    put_u64(&mut attr, 8, config);
    // Bit fields: disabled (0), exclude_kernel (5), exclude_hv (6).
    let mut flags: u64 = 1;
    if exclude_kernel {
        flags |= (1 << 5) | (1 << 6);
    }
    put_u64(&mut attr, 40, flags);
    // SAFETY: `attr` is a valid perf_event_attr of PERF_ATTR_SIZE bytes (the
    // size given in its size field) that outlives the call.
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
