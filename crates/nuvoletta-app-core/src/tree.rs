//! The note tree: notes grouped into folders by virtual path.

use nuvoletta_core::NoteId;
use nuvoletta_storage::NoteSummary;

/// One row of the flattened tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeRow {
    /// A folder (a shared path prefix).
    Folder {
        /// Nesting depth, 0 at the top.
        depth: usize,
        /// The folder's own name.
        name: String,
        /// Full folder path.
        path: String,
    },
    /// A note.
    Note {
        /// Nesting depth, 0 at the top.
        depth: usize,
        /// Label (title, or the last path segment when the title is empty).
        label: String,
        /// Note id.
        id: NoteId,
    },
}

/// Flatten summaries into depth-first rows. Within a folder, its own notes come first, then its
/// subfolders; both are ordered case-insensitively.
#[must_use]
pub fn build_tree(summaries: &[NoteSummary]) -> Vec<TreeRow> {
    let mut items: Vec<(Vec<String>, &NoteSummary)> = summaries
        .iter()
        .map(|s| (s.path.segments().map(str::to_owned).collect(), s))
        .collect();
    items.sort_by_cached_key(|(segs, s)| {
        let folders: Vec<String> = segs[..segs.len() - 1]
            .iter()
            .map(|x| x.to_lowercase())
            .collect();
        (folders, segs.last().map(|l| l.to_lowercase()), s.id)
    });
    let mut rows = Vec::new();
    let mut open: Vec<String> = Vec::new();
    for (segs, s) in &items {
        let folders = &segs[..segs.len() - 1];
        let common = open.iter().zip(folders).take_while(|(a, b)| a == b).count();
        open.truncate(common);
        for (i, name) in folders.iter().enumerate().skip(common) {
            rows.push(TreeRow::Folder {
                depth: i,
                name: name.clone(),
                path: folders[..=i].join("/"),
            });
            open.push(name.clone());
        }
        let label = if s.title.trim().is_empty() {
            segs.last().cloned().unwrap_or_default()
        } else {
            s.title.clone()
        };
        rows.push(TreeRow::Note {
            depth: folders.len(),
            label,
            id: s.id,
        });
    }
    rows
}
