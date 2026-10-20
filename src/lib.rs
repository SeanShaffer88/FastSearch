//! FastSearch — pure logic core: the index, the matcher, persistence.
//!
//! Platform access (walking a directory tree) lives in [`platform`];
//! the C ABI lives in [`ffi`]. Everything here is OS-independent and
//! unit-tested on any platform.
//!
//! Search model (v0.1):
//! - A query with `*` or `?` is a glob, matched against the file NAME.
//! - Otherwise the query is split on whitespace and EVERY token must be a
//!   case-insensitive substring of the name (AND semantics).
//! - Results are sorted by (lowercased name, full path) — deterministic,
//!   so callers and tests see stable ordering.

pub mod ffi;
pub mod platform;

pub const VERSION: &str = "0.1.0";
pub const VERSION_MAJOR: u32 = 0;
pub const VERSION_MINOR: u32 = 1;
pub const VERSION_PATCH: u32 = 0;

pub const FS_OK: i32 = 0;
pub const ERR_INVALID_ARG: i32 = 1;
pub const ERR_NOT_SUPPORTED: i32 = 2;
pub const ERR_NOT_FOUND: i32 = 3;
pub const ERR_ACCESS_DENIED: i32 = 4;
pub const ERR_PLATFORM: i32 = 5;
pub const ERR_INTERNAL: i32 = 6;

// ---------------------------------------------------------------------------
// JSON helpers
// ---------------------------------------------------------------------------

pub fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Index
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    /// Final path component (file or directory name).
    pub name: String,
    pub name_lower: String,
    pub is_dir: bool,
}

impl Entry {
    pub fn new(path: String, is_dir: bool) -> Self {
        let name = path
            .trim_end_matches(['/', '\\'])
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or("")
            .to_string();
        let name_lower = name.to_lowercase();
        Entry {
            path,
            name,
            name_lower,
            is_dir,
        }
    }
    pub fn to_json(&self) -> String {
        format!(
            "{{\"path\":\"{}\",\"name\":\"{}\",\"is_dir\":{}}}",
            json_escape(&self.path),
            json_escape(&self.name),
            self.is_dir
        )
    }
}

#[derive(Clone, Debug, Default)]
pub struct Index {
    pub entries: Vec<Entry>,
}

impl Index {
    /// Build an index from (path, is_dir) pairs. Entries are sorted ONCE,
    /// here, by (lowercase name, path): search is then a pure scan in
    /// sorted order with no per-query sort. (Per-query sorting measured
    /// ~85 ms on a 178k-entry index — path comparisons on long,
    /// common-prefix strings scattered across the heap dominate.)
    pub fn from_paths(paths: Vec<(String, bool)>) -> Self {
        let mut entries: Vec<Entry> = paths.into_iter().map(|(p, d)| Entry::new(p, d)).collect();
        sort_entries(&mut entries);
        Index { entries }
    }

    pub fn search(&self, query: &str, max_results: usize) -> Vec<&Entry> {
        let prepared = PreparedQuery::parse(query);
        let mut hits: Vec<&Entry> = Vec::new();
        // Entries are stored sorted (see from_paths), so hits come out in
        // order and a limited search can stop as soon as it has enough.
        for e in &self.entries {
            if prepared.matches(&e.name_lower) {
                hits.push(e);
                if max_results > 0 && hits.len() >= max_results {
                    break;
                }
            }
        }
        hits
    }

    pub fn search_json(&self, query: &str, max_results: usize) -> String {
        let hits = self.search(query, max_results);
        let mut s = String::from("[");
        for (i, e) in hits.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            s.push_str(&e.to_json());
        }
        s.push(']');
        s
    }

    // -- Persistence -------------------------------------------------------
    // Format "FSIDX1": a magic line, then one line per entry:
    //   F <escaped path>   or   D <escaped path>
    // Escaping: % -> %25, \n -> %0A, \r -> %0D, \t -> %09 (in that order).

    pub fn save(&self) -> String {
        let mut out = String::from("FSIDX1\n");
        for e in &self.entries {
            out.push(if e.is_dir { 'D' } else { 'F' });
            out.push(' ');
            out.push_str(&escape_path(&e.path));
            out.push('\n');
        }
        out
    }

    pub fn load(text: &str) -> Result<Index, String> {
        let mut lines = text.lines();
        match lines.next() {
            Some("FSIDX1") => {}
            _ => return Err("not an FSIDX1 index file".to_string()),
        }
        let mut entries = Vec::new();
        for line in lines {
            if line.is_empty() {
                continue;
            }
            if line.len() < 2 {
                return Err(format!("corrupt index line: {line:.40}"));
            }
            let (kind, rest) = line.split_at(2);
            let is_dir = match kind {
                "D " => true,
                "F " => false,
                _ => return Err(format!("corrupt index line: {line:.40}")),
            };
            entries.push(Entry::new(unescape_path(rest), is_dir));
        }
        sort_entries(&mut entries);
        Ok(Index { entries })
    }
}

/// The one canonical entry order: (lowercase name, full path).
fn sort_entries(entries: &mut [Entry]) {
    entries.sort_by(|a, b| {
        a.name_lower
            .cmp(&b.name_lower)
            .then_with(|| a.path.cmp(&b.path))
    });
}

fn escape_path(p: &str) -> String {
    p.replace('%', "%25")
        .replace('\n', "%0A")
        .replace('\r', "%0D")
        .replace('\t', "%09")
}

fn unescape_path(s: &str) -> String {
    s.replace("%09", "\t")
        .replace("%0D", "\r")
        .replace("%0A", "\n")
        .replace("%25", "%")
}

// ---------------------------------------------------------------------------
// Matching
// ---------------------------------------------------------------------------

/// Does a lowercased name match `query` under the v0.1 search model?
/// Convenience wrapper that prepares the query on every call — hot paths
/// (Index::search) prepare once via [`PreparedQuery`] instead.
pub fn matches_query(name_lower: &str, query: &str) -> bool {
    PreparedQuery::parse(query).matches(name_lower)
}

/// A query parsed ONCE, then matched against many names with zero
/// per-entry allocation. (The first benchmark of the naive per-entry
/// version measured 96 ms per search over 175k entries — the parse and
/// the token lowercasing were being redone for every name.)
pub enum PreparedQuery {
    Empty,
    Glob(Vec<GlobTok>),
    Tokens(Vec<String>),
    /// `*.ext`-style: a single leading star + literal tail -> ends_with.
    Suffix(String),
    /// `name*`-style: literal head + a single trailing star -> starts_with.
    Prefix(String),
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum GlobTok {
    Lit(char),
    Star,
    Quest,
}

impl PreparedQuery {
    pub fn parse(query: &str) -> Self {
        let q = query.trim();
        if q.is_empty() {
            return PreparedQuery::Empty;
        }
        if q.contains('*') || q.contains('?') {
            let lower = q.to_lowercase();
            if let Some(tail) = lower.strip_prefix('*') {
                if !tail.is_empty() && !tail.contains(['*', '?']) {
                    return PreparedQuery::Suffix(tail.to_string());
                }
            }
            if let Some(head) = lower.strip_suffix('*') {
                if !head.is_empty() && !head.contains(['*', '?']) {
                    return PreparedQuery::Prefix(head.to_string());
                }
            }
            let toks = lower
                .chars()
                .map(|c| match c {
                    '*' => GlobTok::Star,
                    '?' => GlobTok::Quest,
                    c => GlobTok::Lit(c),
                })
                .collect();
            PreparedQuery::Glob(toks)
        } else {
            PreparedQuery::Tokens(q.split_whitespace().map(|tok| tok.to_lowercase()).collect())
        }
    }

    pub fn matches(&self, name_lower: &str) -> bool {
        match self {
            PreparedQuery::Empty => false,
            PreparedQuery::Glob(toks) => glob_match_toks(toks, name_lower),
            PreparedQuery::Tokens(toks) => toks.iter().all(|tok| name_lower.contains(tok.as_str())),
            PreparedQuery::Suffix(tail) => name_lower.ends_with(tail.as_str()),
            PreparedQuery::Prefix(head) => name_lower.starts_with(head.as_str()),
        }
    }
}

/// Glob match a prepared pattern against text. Operates on the text's
/// bytes, decoding one UTF-8 char at a time — no allocation.
/// (`?` matches exactly one Unicode char, not one byte.)
pub fn glob_match_toks(pattern: &[GlobTok], text: &str) -> bool {
    // Char start offsets of the text, computed on the fly by decoding.
    fn char_at(b: &[u8], mut i: usize) -> Option<(char, usize)> {
        if i >= b.len() {
            return None;
        }
        let first = b[i];
        let len = if first < 0x80 {
            1
        } else if first >> 5 == 0b110 {
            2
        } else if first >> 4 == 0b1110 {
            3
        } else {
            4
        };
        let end = (i + len).min(b.len());
        let s = std::str::from_utf8(&b[i..end]).ok()?;
        let c = s.chars().next()?;
        i += c.len_utf8();
        Some((c, i))
    }
    let b = text.as_bytes();
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star_p, mut star_t) = (usize::MAX, 0usize);
    loop {
        let cur = char_at(b, ti);
        match cur {
            Some((c, next)) => {
                if pi < pattern.len()
                    && (pattern[pi] == GlobTok::Quest || pattern[pi] == GlobTok::Lit(c))
                {
                    pi += 1;
                    ti = next;
                } else if pi < pattern.len() && pattern[pi] == GlobTok::Star {
                    star_p = pi;
                    star_t = ti;
                    pi += 1;
                } else if star_p != usize::MAX {
                    pi = star_p + 1;
                    // Advance the star's text anchor by one char.
                    star_t = char_at(b, star_t).map(|(_, n)| n).unwrap_or(b.len());
                    ti = star_t;
                } else {
                    return false;
                }
            }
            None => break,
        }
    }
    while pi < pattern.len() && pattern[pi] == GlobTok::Star {
        pi += 1;
    }
    pi == pattern.len()
}

/// Glob match: `*` = any run of chars, `?` = exactly one char.
/// Both arguments must already be lowercased by the caller.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    // PreparedQuery::parse lowercases the pattern and picks the fastest
    // applicable strategy (suffix / prefix / general glob).
    PreparedQuery::parse(pattern).matches(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn idx() -> Index {
        Index::from_paths(vec![
            ("C:\\Docs\\Report 2026.docx".into(), false),
            ("C:\\Docs\\annual report.pdf".into(), false),
            ("C:\\Photos\\IMG_0001.jpg".into(), false),
            ("C:\\Photos\\Reports".into(), true),
            ("C:\\src\\reporter.rs".into(), false),
        ])
    }

    #[test]
    fn substring_tokens_are_anded_case_insensitive() {
        let index = idx();
        let hits = index.search("report", 0);
        assert_eq!(hits.len(), 4); // everything except the photo
        let hits = index.search("annual report", 0);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "annual report.pdf");
        assert!(index.search("", 0).is_empty());
        assert!(index.search("zzz", 0).is_empty());
    }

    #[test]
    fn glob_matching() {
        assert!(glob_match("*.jpg", "img_0001.jpg"));
        assert!(glob_match("img_????.jpg", "img_0001.jpg"));
        assert!(!glob_match("*.png", "img_0001.jpg"));
        assert!(glob_match("report*", "reporter.rs"));
        assert!(glob_match("*", "anything"));
        let i = idx();
        let hits = i.search("*.rs", 0);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "reporter.rs");
    }

    #[test]
    fn limit_and_ordering() {
        let i = idx();
        let hits = i.search("report", 2);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].name, "annual report.pdf");
        let j = i.search_json("report", 1);
        assert!(j.starts_with("[{") && j.ends_with("}]"));
        assert!(j.contains("\"is_dir\":false"));
    }

    #[test]
    fn save_load_roundtrip() {
        let i = idx();
        let text = i.save();
        assert!(text.starts_with("FSIDX1\n"));
        let back = Index::load(&text).unwrap();
        assert_eq!(back.entries, i.entries);
        let weird = Index::from_paths(vec![("C:\\a\tb%c.txt".into(), false)]);
        let back = Index::load(&weird.save()).unwrap();
        assert_eq!(back.entries[0].path, "C:\\a\tb%c.txt");
        assert!(Index::load("nope").is_err());
        assert!(Index::load("FSIDX1\nX bogus\n").is_err());
    }

    #[test]
    fn entry_names() {
        let e = Entry::new("C:\\a\\b\\file.txt".into(), false);
        assert_eq!(e.name, "file.txt");
        let e = Entry::new("/home/u/file.txt".into(), false);
        assert_eq!(e.name, "file.txt");
        assert!(e.to_json().contains("\"name\":\"file.txt\""));
    }
}
