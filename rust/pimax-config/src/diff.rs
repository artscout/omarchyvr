//! Minimal line diff (LCS) for the profile/backup comparison view.

/// One line of a diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffLine {
    /// In both.
    Same(String),
    /// Only in the old text.
    Removed(String),
    /// Only in the new text.
    Added(String),
}

/// Line diff of `old` to `new`.
pub fn diff_lines(old: &str, new: &str) -> Vec<DiffLine> {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let (n, m) = (a.len(), b.len());
    let mut t = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            t[i][j] = if a[i] == b[j] {
                t[i + 1][j + 1] + 1
            } else {
                t[i + 1][j].max(t[i][j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::new();
    while i < n && j < m {
        if a[i] == b[j] {
            out.push(DiffLine::Same(a[i].into()));
            i += 1;
            j += 1;
        } else if t[i + 1][j] >= t[i][j + 1] {
            out.push(DiffLine::Removed(a[i].into()));
            i += 1;
        } else {
            out.push(DiffLine::Added(b[j].into()));
            j += 1;
        }
    }
    out.extend(a[i..].iter().map(|l| DiffLine::Removed((*l).into())));
    out.extend(b[j..].iter().map(|l| DiffLine::Added((*l).into())));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_marks_changed_lines() {
        let d = diff_lines("a\nb\nc\n", "a\nB\nc\nd\n");
        assert_eq!(
            d,
            vec![
                DiffLine::Same("a".into()),
                DiffLine::Removed("b".into()),
                DiffLine::Added("B".into()),
                DiffLine::Same("c".into()),
                DiffLine::Added("d".into()),
            ]
        );
        assert!(diff_lines("x", "x")
            .iter()
            .all(|l| matches!(l, DiffLine::Same(_))));
    }
}
