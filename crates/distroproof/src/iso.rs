//! Écriture d'une image ISO 9660 minimale avec extension Joliet.
//!
//! Sert uniquement au seed cloud-init (NoCloud) : quelques petits fichiers à
//! la racine et un label de volume `cidata`. Joliet conserve les noms en
//! minuscules (`user-data`, `meta-data`), que Linux préfère à la table ISO.

const SECTOR: usize = 2048;

fn both16(v: u16) -> [u8; 4] {
    let le = v.to_le_bytes();
    let be = v.to_be_bytes();
    [le[0], le[1], be[0], be[1]]
}

fn both32(v: u32) -> [u8; 8] {
    let le = v.to_le_bytes();
    let be = v.to_be_bytes();
    [le[0], le[1], le[2], le[3], be[0], be[1], be[2], be[3]]
}

/// Encodage des noms : ISO (a-characters en majuscules) ou Joliet (UCS-2 BE).
#[derive(Clone, Copy)]
enum Names {
    Iso,
    Joliet,
}

impl Names {
    fn file_id(self, name: &str) -> Vec<u8> {
        match self {
            Names::Iso => {
                let upper: String = name
                    .chars()
                    .map(|c| {
                        if c.is_ascii_alphanumeric() || c == '-' {
                            c.to_ascii_uppercase()
                        } else {
                            '_'
                        }
                    })
                    .collect();
                let dotted = if upper.contains('.') {
                    upper
                } else {
                    format!("{upper}.")
                };
                format!("{dotted};1").into_bytes()
            }
            Names::Joliet => ucs2(name),
        }
    }

    fn text(self, s: &str, len: usize) -> Vec<u8> {
        let mut out = match self {
            Names::Iso => s.as_bytes().to_vec(),
            Names::Joliet => ucs2(s),
        };
        while out.len() < len {
            match self {
                Names::Iso => out.push(b' '),
                Names::Joliet => out.extend_from_slice(&[0, b' ']),
            }
        }
        out.truncate(len);
        out
    }
}

fn ucs2(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(u16::to_be_bytes).collect()
}

fn dir_record(extent: u32, size: u32, is_dir: bool, id: &[u8]) -> Vec<u8> {
    let mut len = 33 + id.len();
    if len % 2 == 1 {
        len += 1;
    }
    let mut r = vec![0u8; len];
    r[0] = len as u8;
    r[2..10].copy_from_slice(&both32(extent));
    r[10..18].copy_from_slice(&both32(size));
    // 1er janvier 2000, UTC.
    r[18..25].copy_from_slice(&[100, 1, 1, 0, 0, 0, 0]);
    r[25] = if is_dir { 2 } else { 0 };
    r[28..32].copy_from_slice(&both16(1));
    r[32] = id.len() as u8;
    r[33..33 + id.len()].copy_from_slice(id);
    r
}

fn path_table(root: u32, big_endian: bool) -> Vec<u8> {
    let mut t = vec![0u8; 10];
    t[0] = 1;
    let loc = if big_endian {
        root.to_be_bytes()
    } else {
        root.to_le_bytes()
    };
    t[2..6].copy_from_slice(&loc);
    let parent = if big_endian {
        1u16.to_be_bytes()
    } else {
        1u16.to_le_bytes()
    };
    t[6..8].copy_from_slice(&parent);
    t
}

#[allow(clippy::too_many_arguments)]
fn volume_descriptor(
    kind: u8,
    names: Names,
    label: &str,
    total: u32,
    l_table: u32,
    m_table: u32,
    root: u32,
) -> Vec<u8> {
    let mut d = vec![0u8; SECTOR];
    d[0] = kind;
    d[1..6].copy_from_slice(b"CD001");
    d[6] = 1;
    d[8..40].copy_from_slice(&names.text("LINUX", 32));
    d[40..72].copy_from_slice(&names.text(label, 32));
    d[80..88].copy_from_slice(&both32(total));
    if let Names::Joliet = names {
        // Séquence d'échappement UCS-2 niveau 3.
        d[88..91].copy_from_slice(b"%/E");
    }
    d[120..124].copy_from_slice(&both16(1));
    d[124..128].copy_from_slice(&both16(1));
    d[128..132].copy_from_slice(&both16(SECTOR as u16));
    d[132..140].copy_from_slice(&both32(10));
    d[140..144].copy_from_slice(&l_table.to_le_bytes());
    d[148..152].copy_from_slice(&m_table.to_be_bytes());
    d[156..190].copy_from_slice(&dir_record(root, SECTOR as u32, true, &[0]));
    for range in [190..318, 318..446, 446..574, 574..702] {
        let len = range.len();
        d[range].copy_from_slice(&names.text("", len));
    }
    for range in [702..739, 739..776, 776..813] {
        let len = range.len();
        d[range].copy_from_slice(&names.text("", len));
    }
    // Dates « non précisées » : seize chiffres zéro et fuseau 0.
    for start in [813, 830, 847, 864] {
        d[start..start + 16].copy_from_slice(b"0000000000000000");
    }
    d[881] = 1;
    d
}

fn root_dir(names: Names, root: u32, files: &[(String, u32, u32)]) -> Vec<u8> {
    let mut dir = Vec::new();
    dir.extend(dir_record(root, SECTOR as u32, true, &[0]));
    dir.extend(dir_record(root, SECTOR as u32, true, &[1]));
    for (name, extent, size) in files {
        dir.extend(dir_record(*extent, *size, false, &names.file_id(name)));
    }
    assert!(
        dir.len() <= SECTOR,
        "trop de fichiers pour un seed cloud-init"
    );
    dir.resize(SECTOR, 0);
    dir
}

/// Construit une ISO contenant `files` à la racine.
pub fn build(label: &str, files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut files: Vec<(&str, &[u8])> = files.to_vec();
    files.sort_by(|a, b| a.0.cmp(b.0));

    // 0-15 zone système, 16 PVD, 17 SVD Joliet, 18 terminateur,
    // 19-22 tables de chemins, 23-24 répertoires racine, puis les données.
    let (pvd, svd, term) = (16u32, 17u32, 18u32);
    let (iso_l, iso_m, jol_l, jol_m) = (19u32, 20u32, 21u32, 22u32);
    let (iso_root, jol_root) = (23u32, 24u32);
    let mut next = 25u32;
    let mut layout = Vec::new();
    for (name, data) in &files {
        layout.push((name.to_string(), next, data.len() as u32));
        next += data.len().div_ceil(SECTOR).max(1) as u32;
    }
    let total = next;

    let mut img = vec![0u8; total as usize * SECTOR];
    let mut put = |sector: u32, bytes: &[u8]| {
        let at = sector as usize * SECTOR;
        img[at..at + bytes.len()].copy_from_slice(bytes);
    };
    put(
        pvd,
        &volume_descriptor(1, Names::Iso, label, total, iso_l, iso_m, iso_root),
    );
    put(
        svd,
        &volume_descriptor(2, Names::Joliet, label, total, jol_l, jol_m, jol_root),
    );
    let mut t = vec![0u8; 7];
    t[0] = 255;
    t[1..6].copy_from_slice(b"CD001");
    t[6] = 1;
    put(term, &t);
    put(iso_l, &path_table(iso_root, false));
    put(iso_m, &path_table(iso_root, true));
    put(jol_l, &path_table(jol_root, false));
    put(jol_m, &path_table(jol_root, true));
    put(iso_root, &root_dir(Names::Iso, iso_root, &layout));
    put(jol_root, &root_dir(Names::Joliet, jol_root, &layout));
    for ((_, data), (_, extent, _)) in files.iter().zip(&layout) {
        put(*extent, data);
    }
    img
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_is_consistent() {
        let img = build(
            "cidata",
            &[("user-data", b"#cloud-config\n"), ("meta-data", b"x")],
        );
        assert_eq!(img.len() % SECTOR, 0);
        assert_eq!(&img[16 * SECTOR + 1..16 * SECTOR + 6], b"CD001");
        assert_eq!(&img[16 * SECTOR + 40..16 * SECTOR + 46], b"cidata");
        let total = u32::from_le_bytes(img[16 * SECTOR + 80..16 * SECTOR + 84].try_into().unwrap());
        assert_eq!(total as usize * SECTOR, img.len());
        // meta-data trié avant user-data : secteur 25, puis 26.
        assert_eq!(img[26 * SECTOR], b'#');
        assert_eq!(img[25 * SECTOR], b'x');
        let joliet_root = &img[24 * SECTOR..25 * SECTOR];
        let needle = ucs2("user-data");
        assert!(joliet_root.windows(needle.len()).any(|w| w == needle));
    }
}
