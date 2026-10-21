//! Read-only, queryable differences between filename catalogs. No disk access.
use crate::{Entry, Index, IndexLoadLimits, PreparedQuery};
use std::collections::BTreeMap;

pub const MAX_DETAIL_ROWS: usize = 10_000;
const DETAIL_PATH_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub path: String,
    pub before_folder: Option<bool>,
    pub after_folder: Option<bool>,
}
impl Change {
    pub fn kind(&self) -> &'static str {
        match (self.before_folder, self.after_folder) {
            (None, Some(_)) => "added_to_index",
            (Some(_), None) => "missing_from_index",
            _ => "kind_changed",
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct DiffReport {
    pub added_to_index: u64,
    pub missing_from_index: u64,
    pub kind_changed: u64,
    pub unchanged: u64,
    pub changes: Vec<Change>,
    pub truncated: bool,
}
impl DiffReport {
    pub fn to_json(&self) -> String {
        fn flag(v: Option<bool>) -> &'static str {
            match v {
                None => "null",
                Some(true) => "true",
                Some(false) => "false",
            }
        }
        let rows = self
            .changes
            .iter()
            .map(|c| {
                format!(
            "{{\"path\":\"{}\",\"change\":\"{}\",\"before_folder\":{},\"after_folder\":{}}}",
            crate::json_escape(&c.path), c.kind(), flag(c.before_folder), flag(c.after_folder)
        )
            })
            .collect::<Vec<_>>()
            .join(",");
        format!("{{\"schema_version\":1,\"filesystem_verified\":false,\"added_to_index\":{},\"missing_from_index\":{},\"kind_changed\":{},\"unchanged\":{},\"details_truncated\":{},\"changes\":[{}]}}",
            self.added_to_index, self.missing_from_index, self.kind_changed, self.unchanged, self.truncated, rows)
    }
}

fn catalog(index: &Index) -> Result<BTreeMap<&str, &Entry>, String> {
    if index.entries.len() > IndexLoadLimits::default().max_entries {
        return Err("Comparison index exceeds entry limit".into());
    }
    let mut paths = BTreeMap::new();
    let mut bytes = 0usize;
    for entry in &index.entries {
        bytes = bytes
            .checked_add(entry.path.len())
            .ok_or("Comparison path size overflow")?;
        if bytes > IndexLoadLimits::default().max_bytes {
            return Err("Comparison index exceeds path byte limit".into());
        }
        if let Some(previous) = paths.insert(entry.path.as_str(), entry) {
            if previous.is_dir != entry.is_dir {
                return Err("Ambiguous index: one path is both file and folder".into());
            }
        }
    }
    Ok(paths)
}

/// Compare exact path strings. Blank query means all names; other queries use
/// FastSearch glob/token matching. Matching totals include omitted detail rows.
/// This compares catalogs only, never verifies file existence/content/identity.
pub fn compare(
    before: &Index,
    after: &Index,
    query: &str,
    max_details: usize,
) -> Result<DiffReport, String> {
    let before = catalog(before)?;
    let after = catalog(after)?;
    let filter = PreparedQuery::parse(query);
    let all = query.trim().is_empty();
    let max_details = max_details.min(MAX_DETAIL_ROWS);
    let mut report = DiffReport::default();
    let mut detail_bytes = 0usize;
    let mut left = before.iter().peekable();
    let mut right = after.iter().peekable();
    loop {
        let (old, new) = match (left.peek(), right.peek()) {
            (None, None) => break,
            (Some((a, _)), Some((b, _))) => match a.cmp(b) {
                std::cmp::Ordering::Less => (left.next().map(|(_, e)| *e), None),
                std::cmp::Ordering::Greater => (None, right.next().map(|(_, e)| *e)),
                std::cmp::Ordering::Equal => {
                    (left.next().map(|(_, e)| *e), right.next().map(|(_, e)| *e))
                }
            },
            (Some(_), None) => (left.next().map(|(_, e)| *e), None),
            (None, Some(_)) => (None, right.next().map(|(_, e)| *e)),
        };
        let entry = new.or(old).expect("merge always selects an entry");
        if !all && !filter.matches(&entry.name_lower) {
            continue;
        }
        match (old, new) {
            (None, Some(_)) => report.added_to_index += 1,
            (Some(_), None) => report.missing_from_index += 1,
            (Some(a), Some(b)) if a.is_dir == b.is_dir => {
                report.unchanged += 1;
                continue;
            }
            _ => report.kind_changed += 1,
        }
        if report.changes.len() < max_details
            && entry.path.len() <= DETAIL_PATH_BYTES - detail_bytes
        {
            detail_bytes += entry.path.len();
            report.changes.push(Change {
                path: entry.path.clone(),
                before_folder: old.map(|e| e.is_dir),
                after_folder: new.map(|e| e.is_dir),
            });
        } else {
            report.truncated = true;
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn index(entries: &[(&str, bool)]) -> Index {
        Index::from_paths(entries.iter().map(|(p, d)| ((*p).into(), *d)).collect())
    }
    #[test]
    fn classifies_sorted_changes_and_filters_with_existing_query_language() {
        let a = index(&[
            ("C:/old.dll", false),
            ("C:/same.txt", false),
            ("C:/flip", false),
        ]);
        let b = index(&[
            ("C:/new.dll", false),
            ("C:/same.txt", false),
            ("C:/flip", true),
        ]);
        let r = compare(&a, &b, "", 100).unwrap();
        assert_eq!(
            (
                r.added_to_index,
                r.missing_from_index,
                r.kind_changed,
                r.unchanged
            ),
            (1, 1, 1, 1)
        );
        assert_eq!(
            r.changes
                .iter()
                .map(|c| c.path.as_str())
                .collect::<Vec<_>>(),
            ["C:/flip", "C:/new.dll", "C:/old.dll"]
        );
        assert_eq!(r.changes[0].kind(), "kind_changed");
        let dlls = compare(&a, &b, "*.DLL", 100).unwrap();
        assert_eq!(
            (
                dlls.added_to_index,
                dlls.missing_from_index,
                dlls.kind_changed,
                dlls.unchanged
            ),
            (1, 1, 0, 0)
        );
        assert_eq!(compare(&a, &b, "new dll", 100).unwrap().added_to_index, 1);
        assert_eq!(compare(&a, &a, "", 0).unwrap().unchanged, 3);
    }
    #[test]
    fn limited_details_preserve_totals_and_do_not_change_inputs() {
        let a = index(&[]);
        let b = index(&[("z", false), ("a", false)]);
        let saved = b.save();
        let r = compare(&a, &b, "", 1).unwrap();
        assert_eq!(r.added_to_index, 2);
        assert_eq!(r.changes[0].path, "a");
        assert!(r.truncated);
        assert_eq!(b.save(), saved);
        assert!(compare(&a, &b, "", 0).unwrap().changes.is_empty());
        assert!(!compare(&a, &b, "no-match", 1).unwrap().truncated);
    }
    #[test]
    fn duplicates_unicode_escaping_and_offline_roundtrip() {
        let a = index(&[
            ("C:/雪/old\\n.dll", false),
            ("duplicate", false),
            ("duplicate", false),
        ]);
        let b = Index::load(&index(&[("C:/雪/new\\\".dll", true), ("duplicate", false)]).save())
            .unwrap();
        let r = compare(&a, &b, "", 10).unwrap();
        assert_eq!(r.unchanged, 1);
        assert_eq!(r.missing_from_index, 1);
        assert!(r.to_json().contains("filesystem_verified\":false"));
        assert!(r.to_json().contains("new\\\\\\\".dll"));
        let ambiguous = index(&[("same", true), ("same", false)]);
        assert!(compare(&a, &ambiguous, "", 10)
            .unwrap_err()
            .contains("Ambiguous"));
    }
    #[test]
    fn path_detail_budget_is_explicit_and_totals_survive() {
        let b = index(&[(&"x".repeat(DETAIL_PATH_BYTES + 1), false)]);
        let r = compare(&index(&[]), &b, "", 10).unwrap();
        assert_eq!(r.added_to_index, 1);
        assert!(r.changes.is_empty() && r.truncated);
    }
}
