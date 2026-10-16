//! Markdown tables of docs/INVENTORY.md and docs/LESSONS.md for the Logs & History page: the problems-and-fixes
//! history (INVENTORY section 4), the open issues (5.1), the safety rules (6) and every LESSONS table.
//! A deliberately small parser: headings, pipe tables (with `\|` escapes), other lines kept as text.

/// A pipe table.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Table {
    /// Header cells.
    pub header: Vec<String>,
    /// Body rows (padded to the header width).
    pub rows: Vec<Vec<String>>,
}

impl Table {
    /// Rows containing `needle` in any cell (case-insensitive); all rows for an empty needle.
    pub fn filter(&self, needle: &str) -> Vec<&Vec<String>> {
        let n = needle.trim().to_lowercase();
        self.rows
            .iter()
            .filter(|r| n.is_empty() || r.iter().any(|c| c.to_lowercase().contains(&n)))
            .collect()
    }
}

/// A heading with what follows it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Section {
    /// Heading level (number of `#`).
    pub level: usize,
    /// Heading text.
    pub title: String,
    /// Tables in the section.
    pub tables: Vec<Table>,
    /// Other non-empty lines (bullets, paragraphs).
    pub text: Vec<String>,
}

/// Split one table line into cells (`\|` is a literal pipe), with markup cleaned.
pub fn split_row(line: &str) -> Vec<String> {
    let t = line.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = t.strip_suffix('|').unwrap_or(t);
    let mut cells = Vec::new();
    let mut cur = String::new();
    let mut chars = t.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' && chars.peek() == Some(&'|') {
            cur.push('|');
            chars.next();
        } else if c == '|' {
            cells.push(clean(&cur));
            cur.clear();
        } else {
            cur.push(c);
        }
    }
    cells.push(clean(&cur));
    cells
}

/// Remove bold markers and trim.
pub fn clean(cell: &str) -> String {
    cell.replace("**", "").trim().to_string()
}

fn is_separator(line: &str) -> bool {
    let t = line.trim();
    t.starts_with('|') && t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

/// Parse a markdown text into sections.
pub fn parse(md: &str) -> Vec<Section> {
    let mut out: Vec<Section> = vec![Section::default()];
    let lines: Vec<&str> = md.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let l = lines[i];
        let t = l.trim_start();
        if t.starts_with('#') {
            let level = t.chars().take_while(|c| *c == '#').count();
            out.push(Section {
                level,
                title: t[level..].trim().to_string(),
                ..Default::default()
            });
            i += 1;
            continue;
        }
        if t.starts_with('|') && i + 1 < lines.len() && is_separator(lines[i + 1]) {
            let header = split_row(t);
            let mut rows = Vec::new();
            i += 2;
            while i < lines.len() && lines[i].trim_start().starts_with('|') {
                let mut r = split_row(lines[i]);
                r.resize(header.len().max(r.len()), String::new());
                rows.push(r);
                i += 1;
            }
            if let Some(s) = out.last_mut() {
                s.tables.push(Table { header, rows });
            }
            continue;
        }
        if !t.trim().is_empty() && t.trim() != "---" {
            if let Some(s) = out.last_mut() {
                s.text.push(t.trim_end().to_string());
            }
        }
        i += 1;
    }
    out.retain(|s| !(s.title.is_empty() && s.tables.is_empty() && s.text.is_empty()));
    out
}

/// The first table of the first section whose title starts with `prefix`.
pub fn table_under(sections: &[Section], prefix: &str) -> Option<Table> {
    sections
        .iter()
        .find(|s| s.title.starts_with(prefix))
        .and_then(|s| s.tables.first().cloned())
}

/// INVENTORY section 4: date, problem, root cause, fix, commit.
pub fn history(inventory: &str) -> Option<Table> {
    table_under(&parse(inventory), "4.")
}

/// INVENTORY section 5.1: open issues.
pub fn open_issues(inventory: &str) -> Option<Table> {
    table_under(&parse(inventory), "5.1")
}

/// INVENTORY section 6: the safety rules as (subsection, lines).
pub fn safety_rules(inventory: &str) -> Vec<(String, Vec<String>)> {
    let secs = parse(inventory);
    let start = secs.iter().position(|s| s.title.starts_with("6."));
    let Some(start) = start else { return Vec::new() };
    secs[start + 1..]
        .iter()
        .take_while(|s| s.level > 2)
        .map(|s| {
            let mut lines = s.text.clone();
            for t in &s.tables {
                for r in &t.rows {
                    lines.push(r.iter().filter(|c| !c.is_empty()).cloned().collect::<Vec<_>>().join(" | "));
                }
            }
            (s.title.clone(), lines)
        })
        .collect()
}

/// Every LESSONS table with its section title.
pub fn lessons(lessons_md: &str) -> Vec<(String, Table)> {
    parse(lessons_md)
        .into_iter()
        .flat_map(|s| {
            let title = s.title.clone();
            s.tables.into_iter().map(move |t| (title.clone(), t))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const INV: &str = "# Inv\n\n## 4. Problems and fixes history\n\nDates are local.\n\n| date | problem | root cause | fix | commit |\n|---|---|---|---|---|\n| 2026-10-13 18:20 | detach loop | no keepalive | **wake** + 0x39 | 3a56e4f |\n| 2026-10-14 | a \\| b | x | y |\n\n---\n\n## 5. Open\n\n### 5.1 Open issues (from STATUS.md)\n\n| # | issue | state | what is needed |\n|---|---|---|---|\n| 1 | 6DoF | phase 1 | walk |\n\n## 6. Safety rules a GUI must respect\n\n### 6.1 Headset protocol (HID)\n\n- Send only bytes seen in Windows captures.\n\n### 6.2 Display and NVIDIA\n\n- NEVER force a DRM hotplug.\n";

    #[test]
    fn inventory_sections() {
        let h = history(INV).unwrap();
        assert_eq!(h.header, vec!["date", "problem", "root cause", "fix", "commit"]);
        assert_eq!(h.rows.len(), 2);
        assert_eq!(h.rows[0][3], "wake + 0x39");
        assert_eq!(h.rows[1][1], "a | b", "escaped pipe");
        assert_eq!(h.rows[1].len(), 5, "short rows are padded");
        assert_eq!(h.filter("KEEPALIVE").len(), 1);
        assert_eq!(h.filter("").len(), 2);
        let o = open_issues(INV).unwrap();
        assert_eq!(o.rows[0], vec!["1", "6DoF", "phase 1", "walk"]);
        let s = safety_rules(INV);
        assert_eq!(s.len(), 2);
        assert_eq!(s[1].0, "6.2 Display and NVIDIA");
        assert_eq!(s[1].1, vec!["- NEVER force a DRM hotplug."]);
    }

    #[test]
    fn the_real_documents_parse() {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs");
        let (Ok(inv), Ok(les)) = (std::fs::read_to_string(base.join("INVENTORY.md")), std::fs::read_to_string(base.join("LESSONS.md"))) else {
            return; // docs not next to the crate (packaged build)
        };
        let h = history(&inv).expect("INVENTORY section 4");
        assert!(h.rows.len() > 20, "{}", h.rows.len());
        assert_eq!(h.header.len(), 5);
        assert!(open_issues(&inv).expect("5.1").rows.len() >= 10);
        assert!(!safety_rules(&inv).is_empty());
        let l = lessons(&les);
        let rows: usize = l.iter().map(|(_, t)| t.rows.len()).sum();
        assert!(rows >= 21, "LESSONS rows 1-21: {rows}");
        assert_eq!(l[0].0, "Execution order and exclusivity");
        assert_eq!(l[0].1.rows[0][0], "1");
    }
}
