//! Automatic resolution of two heads (`SYNC_PROTOCOL.md` section 7.2): metadata-only 3-way merge
//! on the whole-field level. There is no text-level merge: if both sides changed the body to
//! different values, or both changed the same metadata field to different values, the result is
//! `None` and the conflict stays visible.

use nuvoletta_crypto::NotePayload;

/// Whether two revisions have the same title, path, tags and body (timestamps are
/// informational and ignored).
#[must_use]
pub fn same_content(a: &NotePayload, b: &NotePayload) -> bool {
    a.title == b.title && a.path == b.path && a.tags == b.tags && a.body == b.body
}

fn pick<T: PartialEq + Clone>(base: &T, a: &T, b: &T) -> Option<T> {
    match (a != base, b != base) {
        (true, true) => (a == b).then(|| a.clone()),
        (true, false) => Some(a.clone()),
        (false, true) => Some(b.clone()),
        (false, false) => Some(base.clone()),
    }
}

/// Three-way merge of `a` and `b` against their common ancestor `base`. The result is a pure
/// function of the three inputs (so devices merging concurrently write equal content):
/// timestamps are `min(created_at)` and `max(modified_at)`.
#[must_use]
pub fn three_way(base: &NotePayload, a: &NotePayload, b: &NotePayload) -> Option<NotePayload> {
    if a.note_id != b.note_id || a.note_id != base.note_id {
        return None;
    }
    Some(NotePayload {
        schema_version: a.schema_version.max(b.schema_version),
        note_id: a.note_id,
        title: pick(&base.title, &a.title, &b.title)?,
        path: pick(&base.path, &a.path, &b.path)?,
        tags: pick(&base.tags, &a.tags, &b.tags)?,
        created_at: a.created_at.min(b.created_at),
        modified_at: a.modified_at.max(b.modified_at),
        body: pick(&base.body, &a.body, &b.body)?,
    })
}

/// Merge of two revisions with identical content: that content, merged timestamps.
#[must_use]
pub fn identical(a: &NotePayload, b: &NotePayload) -> Option<NotePayload> {
    if a.note_id != b.note_id || !same_content(a, b) {
        return None;
    }
    Some(NotePayload {
        schema_version: a.schema_version.max(b.schema_version),
        note_id: a.note_id,
        title: a.title.clone(),
        path: a.path.clone(),
        tags: a.tags.clone(),
        created_at: a.created_at.min(b.created_at),
        modified_at: a.modified_at.max(b.modified_at),
        body: a.body.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(title: &str, path: &str, tags: &[&str], body: &str) -> NotePayload {
        NotePayload {
            schema_version: 1,
            note_id: [1; 16],
            title: title.into(),
            path: path.into(),
            tags: tags.iter().map(|t| (*t).to_owned()).collect(),
            created_at: 10,
            modified_at: 20,
            body: body.into(),
        }
    }

    #[test]
    fn rename_and_body_edit_merge() {
        let base = p("t", "a", &[], "body");
        let m = three_way(&base, &p("t", "b/a", &[], "body"), &p("t", "a", &[], "new")).unwrap();
        assert_eq!((m.path.as_str(), m.body.as_str()), ("b/a", "new"));
    }

    #[test]
    fn tags_and_title_merge_but_same_field_conflicts() {
        let base = p("t", "a", &[], "x");
        assert!(three_way(&base, &p("t2", "a", &[], "x"), &p("t", "a", &["k"], "x")).is_some());
        assert!(three_way(&base, &p("t2", "a", &[], "x"), &p("t3", "a", &[], "x")).is_none());
        assert!(three_way(&base, &p("t", "a", &[], "1"), &p("t", "a", &[], "2")).is_none());
    }

    #[test]
    fn equal_changes_do_not_conflict() {
        let base = p("t", "a", &[], "x");
        assert!(three_way(&base, &p("t", "a", &[], "same"), &p("t", "a", &[], "same")).is_some());
    }

    #[test]
    fn merge_is_symmetric() {
        let base = p("t", "a", &[], "x");
        let (a, b) = (p("t", "b", &[], "x"), p("t", "a", &["k"], "y"));
        assert_eq!(three_way(&base, &a, &b), three_way(&base, &b, &a));
    }

    #[test]
    fn identical_content_merges_with_merged_timestamps() {
        let mut a = p("t", "a", &[], "x");
        let mut b = a.clone();
        a.modified_at = 5;
        b.modified_at = 9;
        b.created_at = 3;
        let m = identical(&a, &b).unwrap();
        assert_eq!((m.created_at, m.modified_at), (3, 9));
        assert!(identical(&a, &p("t", "a", &[], "y")).is_none());
    }
}
