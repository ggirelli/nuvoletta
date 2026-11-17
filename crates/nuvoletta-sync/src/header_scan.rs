//! Finding vault headers on a provider, for a device that is joining a vault or adopting a newer
//! header (`docs/ADR/0008-device-join-and-header-adoption.md`, `SYNC_PROTOCOL.md` section 2).
//!
//! Only `h/` is listed and only header-sized files are read, so nothing else on the provider is
//! touched before the user has proved they know a secret of the vault. What is found is
//! **untrusted bytes**: a file name proves nothing, and a header that parses has not been
//! authenticated. Authentication needs the user's secret and happens in
//! `nuvoletta_storage::Secret::open`; this module only sorts the candidates.

use crate::error::Result;
use crate::names::{Class, RemoteName, classify};
use crate::provider::{Provider, ProviderError};
use nuvoletta_crypto::VaultHeader;
use nuvoletta_crypto::header::HEADER_LEN;
use nuvoletta_crypto::keys::VAULT_ID_LEN;

/// Most header files read in one scan: a bound against a hostile or huge `h/` directory.
const MAX_FILES: usize = 256;

/// Why a file under `h/` was not used as a candidate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IgnoredHeader {
    /// File name relative to the sync root.
    pub name: String,
    /// What was wrong, in words that carry no secret.
    pub reason: &'static str,
}

/// The candidates for one vault, newest generation first. Two different headers of the same
/// generation are both kept (a lost publication race, or a conflict copy); identical bytes are
/// listed once.
#[derive(Clone, Debug)]
pub struct VaultGroup {
    /// The `vault_id` the candidates claim (unauthenticated until one is opened).
    pub vault_id: [u8; VAULT_ID_LEN],
    /// Parsed candidates, highest generation first.
    pub candidates: Vec<VaultHeader>,
}

impl VaultGroup {
    /// The highest generation among the candidates.
    #[must_use]
    pub fn newest_generation(&self) -> u32 {
        self.candidates.first().map_or(0, VaultHeader::generation)
    }

    /// The candidates of one generation.
    pub fn at_generation(&self, generation: u32) -> impl Iterator<Item = &VaultHeader> {
        self.candidates
            .iter()
            .filter(move |h| h.generation() == generation)
    }
}

/// Everything found under `h/`.
#[derive(Clone, Debug, Default)]
pub struct HeaderScan {
    /// One group per claimed `vault_id`, in a stable order (by id).
    pub groups: Vec<VaultGroup>,
    /// Files that were skipped, as a diagnostic.
    pub ignored: Vec<IgnoredHeader>,
}

/// List `h/` and parse every header-shaped file.
///
/// A file is skipped (and listed in [`HeaderScan::ignored`]) if it is not exactly
/// [`HEADER_LEN`] bytes, does not parse (length, magic, version, KDF ranges, generation), or
/// names a generation other than the one it contains. Stray copies of a header (a synced
/// folder's conflict copy) are considered by their content, like the original.
///
/// # Errors
/// Provider failures while listing or reading.
pub fn scan_headers<P: Provider + ?Sized>(provider: &P) -> Result<HeaderScan> {
    let mut scan = HeaderScan::default();
    let mut read = 0usize;
    for entry in provider.list("h")? {
        let named = match classify(&entry.name) {
            Class::Protocol(RemoteName::Header { generation })
            | Class::Copy(RemoteName::Header { generation }) => generation,
            _ => continue,
        };
        let skip = |scan: &mut HeaderScan, reason| {
            scan.ignored.push(IgnoredHeader {
                name: entry.name.clone(),
                reason,
            });
        };
        if entry.size != HEADER_LEN as u64 {
            skip(&mut scan, "wrong size");
            continue;
        }
        if read >= MAX_FILES {
            skip(&mut scan, "too many files");
            continue;
        }
        read += 1;
        let bytes = match provider.get(&entry.name) {
            Ok(b) => b,
            Err(ProviderError::NotFound(_)) => continue,
            Err(e) => return Err(e.into()),
        };
        let Ok(header) = VaultHeader::parse(&bytes) else {
            skip(&mut scan, "not a valid header");
            continue;
        };
        if header.generation() != named {
            skip(&mut scan, "name and content disagree");
            continue;
        }
        let id = header.vault_id();
        let group = match scan.groups.iter().position(|g| g.vault_id == id) {
            Some(i) => &mut scan.groups[i],
            None => {
                scan.groups.push(VaultGroup {
                    vault_id: id,
                    candidates: Vec::new(),
                });
                scan.groups.last_mut().expect("just pushed")
            }
        };
        if !group.candidates.iter().any(|c| c == &header) {
            group.candidates.push(header);
        }
    }
    scan.groups.sort_by_key(|g| g.vault_id);
    for g in &mut scan.groups {
        // Highest generation first; the bytes break ties so the order is stable.
        g.candidates.sort_by(|a, b| {
            b.generation()
                .cmp(&a.generation())
                .then_with(|| a.as_bytes().cmp(b.as_bytes()))
        });
    }
    Ok(scan)
}

/// The vault fingerprint shown to the user: the raw `vault_id` as 32 lower-case hex digits in
/// groups of four, so it can be compared by eye with the one on another device. (Nothing is
/// derived or hashed: ADR 0008, O3.)
#[must_use]
pub fn fingerprint(vault_id: &[u8; VAULT_ID_LEN]) -> String {
    let hex: String = vault_id.iter().map(|b| format!("{b:02x}")).collect();
    hex.as_bytes()
        .chunks(4)
        .map(|c| std::str::from_utf8(c).unwrap_or_default())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_is_the_raw_id_in_groups_of_four() {
        let mut id = [0u8; VAULT_ID_LEN];
        for (i, b) in id.iter_mut().enumerate() {
            *b = u8::try_from(i * 17).unwrap();
        }
        assert_eq!(fingerprint(&id), "0011 2233 4455 6677 8899 aabb ccdd eeff");
    }
}
