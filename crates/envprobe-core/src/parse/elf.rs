//! Minimal ELF header reader: the interpreter (`PT_INTERP`).

const PT_INTERP: u32 = 3;

struct Reader<'a> {
    data: &'a [u8],
    big_endian: bool,
}

impl Reader<'_> {
    fn bytes<const N: usize>(&self, off: usize) -> Option<[u8; N]> {
        self.data.get(off..off.checked_add(N)?)?.try_into().ok()
    }

    fn u16(&self, off: usize) -> Option<u16> {
        let b = self.bytes(off)?;
        Some(if self.big_endian {
            u16::from_be_bytes(b)
        } else {
            u16::from_le_bytes(b)
        })
    }

    fn u32(&self, off: usize) -> Option<u32> {
        let b = self.bytes(off)?;
        Some(if self.big_endian {
            u32::from_be_bytes(b)
        } else {
            u32::from_le_bytes(b)
        })
    }

    fn u64(&self, off: usize) -> Option<u64> {
        let b = self.bytes(off)?;
        Some(if self.big_endian {
            u64::from_be_bytes(b)
        } else {
            u64::from_le_bytes(b)
        })
    }
}

/// Path of the dynamic interpreter, `None` for a static binary or a non-ELF file.
pub fn interpreter(data: &[u8]) -> Option<String> {
    if data.get(..4)? != b"\x7fELF" {
        return None;
    }
    let r = Reader {
        data,
        big_endian: *data.get(5)? == 2,
    };
    let is_64 = *data.get(4)? == 2;
    let (phoff, phentsize, phnum) = if is_64 {
        (
            r.u64(0x20)? as usize,
            r.u16(0x36)? as usize,
            r.u16(0x38)? as usize,
        )
    } else {
        (
            r.u32(0x1c)? as usize,
            r.u16(0x2a)? as usize,
            r.u16(0x2c)? as usize,
        )
    };
    for i in 0..phnum {
        let ph = phoff.checked_add(i.checked_mul(phentsize)?)?;
        if r.u32(ph)? != PT_INTERP {
            continue;
        }
        let (offset, size) = if is_64 {
            (r.u64(ph + 8)? as usize, r.u64(ph + 0x20)? as usize)
        } else {
            (r.u32(ph + 4)? as usize, r.u32(ph + 0x10)? as usize)
        };
        let raw = data.get(offset..offset.checked_add(size)?)?;
        let raw = raw.split(|&b| b == 0).next().unwrap_or(raw);
        return Some(String::from_utf8_lossy(raw).into_owned());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Little-endian ELF64 with a single PT_INTERP program header.
    fn elf64(interp: Option<&str>) -> Vec<u8> {
        let mut d = vec![0u8; 64 + 56];
        d[..4].copy_from_slice(b"\x7fELF");
        d[4] = 2;
        d[5] = 1;
        d[0x20..0x28].copy_from_slice(&64u64.to_le_bytes());
        d[0x36..0x38].copy_from_slice(&56u16.to_le_bytes());
        d[0x38..0x3a].copy_from_slice(&1u16.to_le_bytes());
        let p_type = if interp.is_some() { PT_INTERP } else { 1 };
        d[64..68].copy_from_slice(&p_type.to_le_bytes());
        if let Some(path) = interp {
            let off = d.len() as u64;
            d.extend_from_slice(path.as_bytes());
            d.push(0);
            d[72..80].copy_from_slice(&off.to_le_bytes());
            d[96..104].copy_from_slice(&(path.len() as u64 + 1).to_le_bytes());
        }
        d
    }

    #[test]
    fn dynamic_static_and_garbage() {
        assert_eq!(
            interpreter(&elf64(Some("/lib/ld-musl-x86_64.so.1"))).as_deref(),
            Some("/lib/ld-musl-x86_64.so.1")
        );
        assert_eq!(interpreter(&elf64(None)), None);
        assert_eq!(interpreter(b"#!/bin/sh\n"), None);
        assert_eq!(interpreter(b"\x7fELF\x02\x01"), None);
    }
}
