//! A small line diff for the conflict view. Not a patch format: it only has to show a person what
//! differs between two versions of a note.

/// How a line relates to the two versions being compared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffKind {
    /// In both versions.
    Same,
    /// Only in the other (right-hand) version.
    Added,
    /// Only in this device's (left-hand) version.
    Removed,
}

/// One line of a diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    /// How the line relates to the two versions.
    pub kind: DiffKind,
    /// The line, without its terminator.
    pub text: String,
}

/// Longest-common-subsequence table cells above which the diff degrades to "common head and tail
/// kept, the middle replaced" (about 16 MB of table).
const MAX_CELLS: usize = 4_000_000;

fn line(kind: DiffKind, text: &str) -> DiffLine {
    DiffLine {
        kind,
        text: text.to_owned(),
    }
}

/// Line diff from `old` to `new`: removed lines come from `old`, added lines from `new`. Large
/// inputs fall back to a coarser (still correct) diff that keeps the common first and last lines.
#[must_use]
pub fn diff_lines(old: &str, new: &str) -> Vec<DiffLine> {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let head = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let tail = a[head..]
        .iter()
        .rev()
        .zip(b[head..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (ma, mb) = (&a[head..a.len() - tail], &b[head..b.len() - tail]);
    let mut out: Vec<DiffLine> = a[..head].iter().map(|l| line(DiffKind::Same, l)).collect();
    if ma.len().saturating_mul(mb.len()) > MAX_CELLS {
        out.extend(ma.iter().map(|l| line(DiffKind::Removed, l)));
        out.extend(mb.iter().map(|l| line(DiffKind::Added, l)));
    } else {
        out.extend(lcs_diff(ma, mb));
    }
    out.extend(a[a.len() - tail..].iter().map(|l| line(DiffKind::Same, l)));
    out
}

fn lcs_diff(a: &[&str], b: &[&str]) -> Vec<DiffLine> {
    let (n, m) = (a.len(), b.len());
    // t[i][j] = length of the LCS of a[i..] and b[j..].
    let mut t = vec![0u32; (n + 1) * (m + 1)];
    let at = |i: usize, j: usize| i * (m + 1) + j;
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            t[at(i, j)] = if a[i] == b[j] {
                t[at(i + 1, j + 1)] + 1
            } else {
                t[at(i + 1, j)].max(t[at(i, j + 1)])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::new();
    while i < n && j < m {
        if a[i] == b[j] {
            out.push(line(DiffKind::Same, a[i]));
            i += 1;
            j += 1;
        } else if t[at(i + 1, j)] >= t[at(i, j + 1)] {
            out.push(line(DiffKind::Removed, a[i]));
            i += 1;
        } else {
            out.push(line(DiffKind::Added, b[j]));
            j += 1;
        }
    }
    out.extend(a[i..].iter().map(|l| line(DiffKind::Removed, l)));
    out.extend(b[j..].iter().map(|l| line(DiffKind::Added, l)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(d: &[DiffLine]) -> String {
        d.iter()
            .map(|l| match l.kind {
                DiffKind::Same => '=',
                DiffKind::Added => '+',
                DiffKind::Removed => '-',
            })
            .collect()
    }

    #[test]
    fn identical_text_is_all_same() {
        assert_eq!(kinds(&diff_lines("a\nb\n", "a\nb\n")), "==");
        assert!(diff_lines("", "").is_empty());
    }

    #[test]
    fn changed_middle_line_is_removed_then_added() {
        let d = diff_lines("a\nb\nc", "a\nx\nc");
        assert_eq!(kinds(&d), "=-+=");
        assert_eq!(d[1].text, "b");
        assert_eq!(d[2].text, "x");
    }

    #[test]
    fn insertions_and_deletions_are_found() {
        assert_eq!(kinds(&diff_lines("a\nc", "a\nb\nc")), "=+=");
        assert_eq!(kinds(&diff_lines("a\nb\nc", "a\nc")), "=-=");
        assert_eq!(kinds(&diff_lines("", "x\ny")), "++");
        assert_eq!(kinds(&diff_lines("x\ny", "")), "--");
    }

    #[test]
    fn huge_unrelated_inputs_degrade_instead_of_exploding() {
        let a: String = (0..3000).map(|i| format!("a{i}\n")).collect();
        let b: String = (0..3000).map(|i| format!("b{i}\n")).collect();
        let d = diff_lines(&a, &b);
        assert_eq!(d.len(), 6000);
    }
}
