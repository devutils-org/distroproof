//! BPF program types, map types and helpers, probed with minimal `bpf()`
//! calls (the equivalent of `bpftool feature probe`, without libbpf).
//! Every object created is closed immediately: nothing stays loaded.

use crate::probes::kernel;
use crate::sys::{self, BpfInsn, MapCreate, ProgLoad};
use crate::util::{self, errno_name};
use crate::{Section, Value};

const BPF_F_NO_PREALLOC: u32 = 1;
const BPF_F_SLEEPABLE: u32 = 1 << 4;
const PROG_KPROBE: u32 = 2;

/// (name, type, expected_attach_type, prog_flags)
const PROG_TYPES: &[(&str, u32, u32, u32)] = &[
    ("socket_filter", 1, 0, 0),
    ("kprobe", PROG_KPROBE, 0, 0),
    ("sched_cls", 3, 0, 0),
    ("sched_act", 4, 0, 0),
    ("tracepoint", 5, 0, 0),
    ("xdp", 6, 0, 0),
    ("perf_event", 7, 0, 0),
    ("cgroup_skb", 8, 0, 0),
    ("cgroup_sock", 9, 0, 0),
    ("lwt_in", 10, 0, 0),
    ("lwt_out", 11, 0, 0),
    ("lwt_xmit", 12, 0, 0),
    ("sock_ops", 13, 0, 0),
    ("sk_skb", 14, 0, 0),
    ("cgroup_device", 15, 0, 0),
    ("sk_msg", 16, 0, 0),
    ("raw_tracepoint", 17, 0, 0),
    // BPF_CGROUP_INET4_CONNECT
    ("cgroup_sock_addr", 18, 10, 0),
    ("lwt_seg6local", 19, 0, 0),
    ("lirc_mode2", 20, 0, 0),
    ("sk_reuseport", 21, 0, 0),
    ("flow_dissector", 22, 0, 0),
    ("cgroup_sysctl", 23, 0, 0),
    ("raw_tracepoint_writable", 24, 0, 0),
    // BPF_CGROUP_GETSOCKOPT
    ("cgroup_sockopt", 25, 21, 0),
    // BPF_SK_LOOKUP
    ("sk_lookup", 30, 36, 0),
    ("syscall", 31, 0, BPF_F_SLEEPABLE),
    // BPF_NETFILTER
    ("netfilter", 32, 45, 0),
];

/// Types that need a BTF target (a function to attach to) to be loaded.
const PROG_TYPES_BTF: &[&str] = &["tracing", "struct_ops", "ext", "lsm"];

/// (name, type, key_size, value_size, max_entries, flags); max_entries 0 = page size.
const MAP_TYPES: &[(&str, u32, u32, u32, u32, u32)] = &[
    ("hash", 1, 4, 8, 1, 0),
    ("array", 2, 4, 8, 1, 0),
    ("prog_array", 3, 4, 4, 1, 0),
    ("perf_event_array", 4, 4, 4, 1, 0),
    ("percpu_hash", 5, 4, 8, 1, 0),
    ("percpu_array", 6, 4, 8, 1, 0),
    ("stack_trace", 7, 4, 8, 1, 0),
    ("cgroup_array", 8, 4, 4, 1, 0),
    ("lru_hash", 9, 4, 8, 1, 0),
    ("lru_percpu_hash", 10, 4, 8, 1, 0),
    ("lpm_trie", 11, 8, 8, 1, BPF_F_NO_PREALLOC),
    ("devmap", 14, 4, 4, 1, 0),
    ("sockmap", 15, 4, 4, 1, 0),
    ("cpumap", 16, 4, 4, 1, 0),
    ("xskmap", 17, 4, 4, 1, 0),
    ("sockhash", 18, 4, 4, 1, 0),
    ("queue", 22, 0, 4, 1, 0),
    ("stack", 23, 0, 4, 1, 0),
    ("devmap_hash", 25, 4, 4, 1, 0),
    ("ringbuf", 27, 0, 0, 0, 0),
    ("bloom_filter", 30, 0, 4, 1, 0),
    ("user_ringbuf", 31, 0, 0, 0, 0),
];

/// Helpers useful for tracing, tested in a kprobe program.
const HELPERS: &[(&str, i32)] = &[
    ("ktime_get_ns", 5),
    ("trace_printk", 6),
    ("get_current_pid_tgid", 14),
    ("get_current_comm", 16),
    ("perf_event_output", 25),
    ("get_stackid", 27),
    ("override_return", 58),
    ("get_stack", 67),
    ("send_signal", 109),
    ("probe_read_user", 112),
    ("probe_read_kernel", 113),
    ("ktime_get_boot_ns", 125),
    ("ringbuf_output", 130),
    ("get_current_task_btf", 158),
];

const LOG_SIZE: usize = 64 * 1024;

pub fn probe() -> Section {
    let mut s = Section::new("bpf");
    let kern_version = sys::uname()
        .map(|u| kernel::version_code(&u.release))
        .unwrap_or(0);
    let trivial = [BpfInsn::mov_r0(0), BpfInsn::exit()];

    // First call: does the syscall even exist?
    let syscall = load(1, 0, 0, kern_version, &trivial, None);
    if syscall == Err(libc::ENOSYS) {
        s.push(
            "syscall",
            Value::unavailable("ENOSYS: bpf() missing (CONFIG_BPF_SYSCALL)"),
        );
        return s;
    }
    s.push_privileged(
        "syscall",
        match syscall {
            Ok(()) => Value::ok("available"),
            Err(e) => util::errno_value(e),
        },
    );
    s.push("jit", jit());

    for (name, prog_type, attach, flags) in PROG_TYPES {
        let mut log = vec![0u8; LOG_SIZE];
        let res = load(
            *prog_type,
            *attach,
            *flags,
            kern_version,
            &trivial,
            Some(&mut log),
        );
        s.push_privileged(format!("prog.{name}"), interpret_prog(res, &log));
    }
    for name in PROG_TYPES_BTF {
        s.push(
            format!("prog.{name}"),
            Value::unavailable("not probed: needs a BTF target"),
        );
    }

    let page = sys::page_size() as u32;
    for (name, map_type, key, value, max, flags) in MAP_TYPES {
        let req = MapCreate {
            map_type: *map_type,
            key_size: *key,
            value_size: *value,
            max_entries: if *max == 0 { page } else { *max },
            map_flags: *flags,
        };
        let res = sys::bpf_map_create(&req).map(drop);
        s.push_privileged(format!("map.{name}"), interpret_simple(res));
    }

    for (name, id) in HELPERS {
        let prog = [BpfInsn::call(*id), BpfInsn::mov_r0(0), BpfInsn::exit()];
        let mut log = vec![0u8; LOG_SIZE];
        let res = load(PROG_KPROBE, 0, 0, kern_version, &prog, Some(&mut log));
        s.push_privileged(format!("helper.{name}"), interpret_helper(res, &log));
    }
    s
}

fn load(
    prog_type: u32,
    expected_attach_type: u32,
    prog_flags: u32,
    kern_version: u32,
    insns: &[BpfInsn],
    log: Option<&mut [u8]>,
) -> Result<(), i32> {
    sys::bpf_prog_load(ProgLoad {
        prog_type,
        expected_attach_type,
        prog_flags,
        kern_version,
        insns,
        log,
    })
    .map(drop)
}

fn log_text(log: &[u8]) -> String {
    let end = log.iter().position(|&b| b == 0).unwrap_or(log.len());
    String::from_utf8_lossy(&log[..end]).into_owned()
}

/// bpftool convention: EINVAL / EOPNOTSUPP / E2BIG mean "unknown type".
fn unsupported(errno: i32) -> bool {
    matches!(errno, libc::EINVAL | libc::EOPNOTSUPP | libc::E2BIG | 524)
}

fn interpret_simple(res: Result<(), i32>) -> Value {
    match res {
        Ok(()) => Value::ok("supported"),
        Err(e) if unsupported(e) => Value::ok(format!("unsupported ({})", errno_name(e))),
        Err(e) => util::errno_value(e),
    }
}

fn interpret_prog(res: Result<(), i32>, log: &[u8]) -> Value {
    match res {
        Err(libc::EACCES) => {
            let text = log_text(log);
            match text.lines().rev().find(|l| !l.trim().is_empty()) {
                Some(line) => {
                    Value::unavailable(format!("rejected by the verifier: {}", line.trim()))
                }
                None => Value::denied("EACCES"),
            }
        }
        other => interpret_simple(other),
    }
}

fn interpret_helper(res: Result<(), i32>, log: &[u8]) -> Value {
    let text = log_text(log);
    let missing = [
        "invalid func ",
        "unknown func ",
        "program of this type cannot use helper ",
    ];
    match res {
        Ok(()) => Value::ok("supported"),
        Err(_) if missing.iter().any(|m| text.contains(m)) => Value::ok("unsupported"),
        // The verifier got past the helper call before rejecting the program.
        Err(_) if !text.trim().is_empty() => Value::ok("supported"),
        Err(e) => util::errno_value(e),
    }
}

fn jit() -> Value {
    match util::read_trimmed("/proc/sys/net/core/bpf_jit_enable") {
        Ok(v) if v == "0" => Value::ok("disabled"),
        Ok(v) if v == "1" => Value::ok("enabled"),
        Ok(v) if v == "2" => Value::ok("enabled (debug)"),
        Ok(v) => Value::ok(v),
        Err(v) => v,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log(s: &str) -> Vec<u8> {
        let mut v = s.as_bytes().to_vec();
        v.push(0);
        v
    }

    #[test]
    fn helper_interpretation() {
        assert_eq!(interpret_helper(Ok(()), &[0]), Value::ok("supported"));
        assert_eq!(
            interpret_helper(
                Err(libc::EINVAL),
                &log("0: (85) call unknown#999\ninvalid func unknown#999\n")
            ),
            Value::ok("unsupported")
        );
        assert_eq!(
            interpret_helper(
                Err(libc::EACCES),
                &log("0: (85) call bpf_probe_read_kernel#113\nR1 !read_ok\n")
            ),
            Value::ok("supported")
        );
        assert_eq!(
            interpret_helper(Err(libc::EPERM), &[0]),
            Value::denied("EPERM")
        );
    }

    #[test]
    fn prog_interpretation() {
        assert_eq!(
            interpret_prog(Err(libc::EINVAL), &[0]),
            Value::ok("unsupported (EINVAL)")
        );
        assert_eq!(
            interpret_prog(Err(libc::EPERM), &[0]),
            Value::denied("EPERM")
        );
        assert_eq!(
            interpret_prog(Err(libc::EACCES), &[0]),
            Value::denied("EACCES")
        );
    }
}
