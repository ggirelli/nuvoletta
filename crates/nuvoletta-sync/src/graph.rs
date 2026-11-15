//! The per-note version DAG (`SYNC_PROTOCOL.md` section 5.3): vertices are revisions (a PUT's
//! `object_id` or a DELETE's `tombstone_id`), edges point to parents. Heads are vertices that are
//! nobody's parent. The graph is a deterministic function of the set of verified entries.

use crate::ids::{DeviceId, Hash32, RevId};
use crate::log::{KIND_DELETE, KIND_PUT};
use minicbor::{Decode, Encode};
use std::collections::{BTreeMap, BTreeSet};

/// Which log entry introduced a vertex. `seq == 0` marks an own entry not yet sealed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Encode, Decode)]
#[cbor(array)]
pub struct Origin {
    /// Writer device.
    #[n(0)]
    pub device: DeviceId,
    /// Sequence number in that device's log (0 = own, unsealed).
    #[n(1)]
    pub seq: u64,
}

/// A revision in the DAG.
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
#[cbor(map)]
pub struct Vertex {
    /// Revision id.
    #[n(0)]
    pub rev: RevId,
    /// [`KIND_PUT`] or [`KIND_DELETE`].
    #[n(1)]
    pub kind: u8,
    /// Parents, sorted and deduplicated.
    #[n(2)]
    pub parents: Vec<RevId>,
    /// Hash of the object file (PUT only).
    #[n(3)]
    #[cbor(skip_if = "Option::is_none")]
    pub ct_hash: Option<Hash32>,
    /// Entries that introduced it (several only for identical re-emissions after a fork).
    #[n(4)]
    pub origins: Vec<Origin>,
    /// Time of the first introducing entry (informational, display order only).
    #[n(5)]
    pub time: i64,
}

impl Vertex {
    /// Whether this vertex is a tombstone.
    #[must_use]
    pub fn is_delete(&self) -> bool {
        self.kind == KIND_DELETE
    }

    /// Whether this vertex is a content revision.
    #[must_use]
    pub fn is_put(&self) -> bool {
        self.kind == KIND_PUT
    }

    fn same_definition(&self, other: &Self) -> bool {
        self.kind == other.kind && self.parents == other.parents && self.ct_hash == other.ct_hash
    }
}

/// Result of adding a vertex.
#[derive(Debug, PartialEq, Eq)]
pub enum Insert {
    /// A new vertex.
    New,
    /// An identical re-introduction (same kind, parents, ct_hash): only the origin was added.
    Same,
    /// The id is already defined differently: the entry is invalid.
    Conflict,
}

/// The DAG of one note.
#[derive(Clone, Debug, Default, PartialEq, Eq, Encode, Decode)]
#[cbor(map)]
pub struct NoteGraph {
    /// All known vertices by revision id.
    #[n(0)]
    pub vertices: BTreeMap<RevId, Vertex>,
}

impl NoteGraph {
    /// Add a vertex (or a repeated introduction of one).
    pub fn insert(&mut self, v: Vertex) -> Insert {
        match self.vertices.get_mut(&v.rev) {
            None => {
                self.vertices.insert(v.rev, v);
                Insert::New
            }
            Some(old) if old.same_definition(&v) => {
                for o in v.origins {
                    if !old.origins.contains(&o) {
                        old.origins.push(o);
                        old.origins.sort();
                    }
                }
                Insert::Same
            }
            Some(_) => Insert::Conflict,
        }
    }

    /// Heads: vertices that are not a parent of any vertex, in revision-id order.
    #[must_use]
    pub fn heads(&self) -> Vec<RevId> {
        let parents: BTreeSet<&RevId> = self.vertices.values().flat_map(|v| &v.parents).collect();
        self.vertices
            .keys()
            .filter(|r| !parents.contains(r))
            .copied()
            .collect()
    }

    /// Heads in the cosmetic display order `(time, first origin device, rev)` of section 7.3.
    #[must_use]
    pub fn heads_display_order(&self) -> Vec<RevId> {
        let mut h = self.heads();
        h.sort_by_key(|r| {
            let v = &self.vertices[r];
            (v.time, v.origins.first().map(|o| o.device), *r)
        });
        h
    }

    /// All known strict ancestors of `rev` (parents, their parents, ...).
    #[must_use]
    pub fn ancestors(&self, rev: &RevId) -> BTreeSet<RevId> {
        let mut out = BTreeSet::new();
        let mut stack: Vec<RevId> = self
            .vertices
            .get(rev)
            .map(|v| v.parents.clone())
            .unwrap_or_default();
        while let Some(r) = stack.pop() {
            if out.insert(r)
                && let Some(v) = self.vertices.get(&r)
            {
                stack.extend(v.parents.iter().copied());
            }
        }
        out
    }

    /// The unique maximal common ancestor of two revisions, if there is exactly one (a criss-cross
    /// history has several, and then there is no safe 3-way base).
    #[must_use]
    pub fn common_base(&self, a: &RevId, b: &RevId) -> Option<RevId> {
        let aa = self.ancestors(a);
        let bb = self.ancestors(b);
        let common: BTreeSet<RevId> = aa.intersection(&bb).copied().collect();
        let dominated: BTreeSet<RevId> = common.iter().flat_map(|c| self.ancestors(c)).collect();
        let mut maximal = common.iter().filter(|c| !dominated.contains(c));
        let first = maximal.next()?;
        if maximal.next().is_some() {
            return None;
        }
        Some(*first)
    }

    /// Remove the introductions by `device` at sequence numbers `>= from_seq` (fork freeze,
    /// section 10.4). Vertices left without any origin disappear. Returns the removed revisions.
    pub fn remove_origins(&mut self, device: &DeviceId, from_seq: u64) -> Vec<RevId> {
        let mut gone = Vec::new();
        for v in self.vertices.values_mut() {
            v.origins
                .retain(|o| !(o.device == *device && o.seq >= from_seq && o.seq != 0));
            if v.origins.is_empty() {
                gone.push(v.rev);
            }
        }
        for r in &gone {
            self.vertices.remove(r);
        }
        gone
    }

    /// Replace an unsealed own origin (`seq == 0`) by its assigned sequence number.
    pub fn assign_seq(&mut self, rev: &RevId, device: &DeviceId, seq: u64) {
        if let Some(v) = self.vertices.get_mut(rev) {
            for o in &mut v.origins {
                if o.device == *device && o.seq == 0 {
                    o.seq = seq;
                }
            }
            v.origins.sort();
            v.origins.dedup();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::Id16;

    fn r(n: u8) -> RevId {
        Id16([n; 16])
    }

    fn v(rev: u8, parents: &[u8]) -> Vertex {
        Vertex {
            rev: r(rev),
            kind: KIND_PUT,
            parents: parents.iter().map(|p| r(*p)).collect(),
            ct_hash: Some(Hash32([rev; 32])),
            origins: vec![Origin {
                device: r(100),
                seq: u64::from(rev),
            }],
            time: 0,
        }
    }

    fn graph(vs: &[(u8, &[u8])]) -> NoteGraph {
        let mut g = NoteGraph::default();
        for (rev, ps) in vs {
            g.insert(v(*rev, ps));
        }
        g
    }

    #[test]
    fn heads_and_ancestors() {
        let g = graph(&[(1, &[]), (2, &[1]), (3, &[1]), (4, &[2, 3])]);
        assert_eq!(g.heads(), vec![r(4)]);
        assert_eq!(g.ancestors(&r(4)), [r(1), r(2), r(3)].into());
        let g = graph(&[(1, &[]), (2, &[1]), (3, &[1])]);
        assert_eq!(g.heads(), vec![r(2), r(3)]);
        assert_eq!(g.common_base(&r(2), &r(3)), Some(r(1)));
    }

    #[test]
    fn heads_do_not_depend_on_insertion_order() {
        let a = graph(&[(1, &[]), (2, &[1]), (3, &[1])]);
        let b = graph(&[(3, &[1]), (2, &[1]), (1, &[])]);
        assert_eq!(a.heads(), b.heads());
    }

    #[test]
    fn missing_parents_are_fine() {
        let g = graph(&[(2, &[1])]);
        assert_eq!(g.heads(), vec![r(2)]);
    }

    #[test]
    fn criss_cross_has_no_unique_base() {
        let g = graph(&[(1, &[]), (2, &[1]), (3, &[1]), (4, &[2, 3]), (5, &[2, 3])]);
        assert_eq!(g.common_base(&r(4), &r(5)), None);
    }

    #[test]
    fn reintroduction_adds_an_origin_and_redefinition_conflicts() {
        let mut g = NoteGraph::default();
        assert_eq!(g.insert(v(1, &[])), Insert::New);
        let mut again = v(1, &[]);
        again.origins = vec![Origin {
            device: r(101),
            seq: 3,
        }];
        assert_eq!(g.insert(again), Insert::Same);
        assert_eq!(g.vertices[&r(1)].origins.len(), 2);
        assert_eq!(g.insert(v(1, &[9])), Insert::Conflict);
    }

    #[test]
    fn freezing_removes_only_the_late_origins() {
        let mut g = graph(&[(1, &[]), (2, &[1]), (3, &[2])]);
        // all introduced by device 100 with seq == rev
        let gone = g.remove_origins(&r(100), 3);
        assert_eq!(gone, vec![r(3)]);
        assert_eq!(g.heads(), vec![r(2)]);
    }
}
