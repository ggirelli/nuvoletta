//! The strict remote name grammar and stray-file classification (`SYNC_PROTOCOL.md` sections 2
//! and 8).
//!
//! ```text
//! h/<header_gen 8 hex>.nvhdr
//! d/<device_id 32 hex>/l/<first_seq 16 hex>.nvlog
//! o/<object_id[0..2] hex>/<object_id 32 hex>.nvobj
//! s/<device_id 32 hex>/<object_id 32 hex>.nvsnap      (reserved)
//! ```
//!
//! All hex is lower case, so case-insensitive filesystems cannot make two names equal.

use crate::ids::{DeviceId, Id16, hex_decode};

/// Prefix of temporary upload names; never read by peers.
pub const TMP_PREFIX: &str = ".tmp-";

/// A protocol file identity derived from a name that matches the grammar.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RemoteName {
    /// A vault header generation.
    Header {
        /// `header_gen`.
        generation: u32,
    },
    /// A log segment of a device.
    Segment {
        /// Writer device.
        device: DeviceId,
        /// Sequence number of the first entry (as claimed by the name).
        first_seq: u64,
    },
    /// A note revision object.
    Object {
        /// The revision id (`object_id`).
        id: Id16,
    },
    /// A snapshot (reserved, post-MVP; never read).
    Snapshot {
        /// Writer device.
        device: DeviceId,
        /// Snapshot object id.
        id: Id16,
    },
}

impl RemoteName {
    /// The canonical relative path of this name.
    #[must_use]
    pub fn path(&self) -> String {
        match self {
            Self::Header { generation } => format!("h/{generation:08x}.nvhdr"),
            Self::Segment { device, first_seq } => {
                format!("d/{device}/l/{first_seq:016x}.nvlog")
            }
            Self::Object { id } => format!("o/{}/{id}.nvobj", id.shard()),
            Self::Snapshot { device, id } => format!("s/{device}/{id}.nvsnap"),
        }
    }

    /// Directory holding a device's log segments.
    #[must_use]
    pub fn segment_dir(device: &DeviceId) -> String {
        format!("d/{device}/l")
    }
}

/// How a listed name is treated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// Matches the strict grammar: a protocol file.
    Protocol(RemoteName),
    /// A stray file in a protocol directory whose name starts with the id the grammar requires
    /// there followed by a non-hex character, with the expected extension. Treated as a copy of
    /// the named file if its content authenticates (section 8).
    Copy(RemoteName),
    /// One of the writers' temporary files (`.tmp-` prefix).
    Temp,
    /// Anything else: listed in diagnostics, never read as truth, never deleted.
    Stray,
}

fn is_hex_digit(b: u8) -> bool {
    b.is_ascii_hexdigit()
}

/// Match `file` as `<idlen hex><rest>.<ext>`. Returns the hex prefix and whether `rest` is empty.
fn split_file<'a>(file: &'a str, idlen: usize, ext: &str) -> Option<(&'a str, bool)> {
    let stem = file.strip_suffix(ext)?.strip_suffix('.')?;
    let b = stem.as_bytes();
    if b.len() < idlen
        || !b[..idlen]
            .iter()
            .all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
    {
        return None;
    }
    if b.len() == idlen {
        return Some((&stem[..idlen], true));
    }
    if is_hex_digit(b[idlen]) {
        return None; // longer hex run, or upper-case hex: not a copy of anything
    }
    Some((&stem[..idlen], false))
}

/// Classify a path relative to the sync root.
#[must_use]
pub fn classify(path: &str) -> Class {
    let parts: Vec<&str> = path.split('/').collect();
    if parts.iter().any(|p| p.is_empty()) {
        return Class::Stray;
    }
    if parts.last().is_some_and(|f| f.starts_with(TMP_PREFIX)) {
        return Class::Temp;
    }
    let wrap = |name: RemoteName, exact: bool| {
        if exact {
            Class::Protocol(name)
        } else {
            Class::Copy(name)
        }
    };
    match parts.as_slice() {
        ["h", file] => split_file(file, 8, "nvhdr")
            .and_then(|(h, exact)| {
                let mut b = [0u8; 4];
                hex_decode(h, &mut b)?;
                Some(wrap(
                    RemoteName::Header {
                        generation: u32::from_be_bytes(b),
                    },
                    exact,
                ))
            })
            .unwrap_or(Class::Stray),
        ["d", dev, "l", file] => {
            let (Some(device), Some((h, exact))) =
                (Id16::from_hex(dev), split_file(file, 16, "nvlog"))
            else {
                return Class::Stray;
            };
            let mut b = [0u8; 8];
            if hex_decode(h, &mut b).is_none() {
                return Class::Stray;
            }
            let first_seq = u64::from_be_bytes(b);
            if first_seq == 0 {
                return Class::Stray;
            }
            wrap(RemoteName::Segment { device, first_seq }, exact)
        }
        ["o", shard, file] => {
            let Some((h, exact)) = split_file(file, 32, "nvobj") else {
                return Class::Stray;
            };
            match Id16::from_hex(h) {
                Some(id) if id.shard() == *shard => wrap(RemoteName::Object { id }, exact),
                _ => Class::Stray,
            }
        }
        ["s", dev, file] => {
            let (Some(device), Some((h, exact))) =
                (Id16::from_hex(dev), split_file(file, 32, "nvsnap"))
            else {
                return Class::Stray;
            };
            match Id16::from_hex(h) {
                Some(id) => wrap(RemoteName::Snapshot { device, id }, exact),
                None => Class::Stray,
            }
        }
        _ => Class::Stray,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev() -> String {
        "0a".repeat(16)
    }

    #[test]
    fn canonical_paths_round_trip() {
        let id = Id16([0xab; 16]);
        for n in [
            RemoteName::Header { generation: 7 },
            RemoteName::Segment {
                device: Id16([0x0a; 16]),
                first_seq: 42,
            },
            RemoteName::Object { id },
            RemoteName::Snapshot {
                device: Id16([1; 16]),
                id,
            },
        ] {
            assert_eq!(classify(&n.path()), Class::Protocol(n), "{}", n.path());
        }
    }

    #[test]
    fn copies_need_a_non_hex_separator_and_the_expected_extension() {
        let seg = format!("d/{}/l/0000000000000007", dev());
        let want = RemoteName::Segment {
            device: Id16([0x0a; 16]),
            first_seq: 7,
        };
        assert_eq!(
            classify(&format!("{seg} (conflicted copy).nvlog")),
            Class::Copy(want)
        );
        assert_eq!(classify(&format!("{seg}-1.nvlog")), Class::Copy(want));
        for bad in [
            format!("{seg}0.nvlog"),
            format!("{seg}A.nvlog"),
            format!("{seg}.nvlog.tmp"),
            format!("{seg} (1).nvobj"),
            format!("d/{}/l/.sync-conflict-20240101.nvlog", dev()),
            format!("d/{}/l/0000000000000000.nvlog", dev()),
            format!("d/{}/l/{}.nvlog", dev(), "A".repeat(16)),
        ] {
            assert_eq!(classify(&bad), Class::Stray, "{bad}");
        }
    }

    #[test]
    fn object_shard_must_match_the_id() {
        let id = "ab".repeat(16);
        assert!(matches!(
            classify(&format!("o/ab/{id}.nvobj")),
            Class::Protocol(_)
        ));
        assert_eq!(classify(&format!("o/cd/{id}.nvobj")), Class::Stray);
        assert!(matches!(
            classify(&format!("o/ab/{id} (1).nvobj")),
            Class::Copy(_)
        ));
    }

    #[test]
    fn temp_and_hostile_names() {
        assert_eq!(classify("d/x/l/.tmp-abc"), Class::Temp);
        for bad in [
            "",
            "h",
            "h/../x",
            "../h/00000001.nvhdr",
            "h//00000001.nvhdr",
            "h/00000001.nvhdr/",
            "h/0000000G.nvhdr",
            "h/00000001.NVHDR",
            ".DS_Store",
            "d/not-a-device/l/0000000000000001.nvlog",
            "h/0000000001.nvhdr",
        ] {
            assert_eq!(classify(bad), Class::Stray, "{bad}");
        }
    }
}
