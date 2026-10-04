//! The directory picker on the new-chat screen: fuzzy over directories you've
//! used before, or plain path completion once the query looks like a path.

use crate::data;
use std::path::{Path, PathBuf};

const MAX_ITEMS: usize = 200;

pub struct DirPicker {
    pub query: String,
    pub items: Vec<PathBuf>,
    pub sel: usize,
    /// Every directory worth offering, most relevant first.
    known: Vec<PathBuf>,
    /// The directory the chat would start in now; relative paths resolve against it.
    base: PathBuf,
}

impl DirPicker {
    pub fn new(query: &str, known: Vec<PathBuf>, base: PathBuf) -> Self {
        let mut p = DirPicker { query: query.into(), items: Vec::new(), sel: 0, known, base };
        p.refresh();
        p
    }

    pub fn refresh(&mut self) {
        self.items = if looks_like_path(&self.query) { self.children() } else { self.fuzzy() };
        self.items.truncate(MAX_ITEMS);
        self.sel = 0;
    }

    pub fn selected(&self) -> Option<&PathBuf> {
        self.items.get(self.sel)
    }

    pub fn move_by(&mut self, d: isize) {
        let n = self.items.len() as isize;
        if n > 0 {
            self.sel = (self.sel as isize + d).rem_euclid(n) as usize;
        }
    }

    /// Tab: take the highlighted directory into the query and list its children.
    pub fn descend(&mut self) {
        if let Some(d) = self.selected() {
            self.query = format!("{}/", data::tilde(d).trim_end_matches('/'));
            self.refresh();
        }
    }

    /// Ctrl-w / Shift-Tab: drop the last path component.
    pub fn up(&mut self) {
        let t = self.query.trim_end_matches('/');
        let cut = t.rfind('/').map(|i| i + 1).unwrap_or(0);
        self.query.truncate(cut);
        self.refresh();
    }

    /// What Enter would pick: the highlighted row, or the typed path itself.
    pub fn resolve(&self) -> Option<PathBuf> {
        self.selected().cloned().or_else(|| Some(self.abs(&self.query)).filter(|p| p.is_dir()))
    }

    fn abs(&self, s: &str) -> PathBuf {
        let p = data::expand_tilde(s);
        if p.is_absolute() { p } else { self.base.join(p) }
    }

    fn fuzzy(&self) -> Vec<PathBuf> {
        let q = self.query.to_lowercase();
        if q.is_empty() {
            // The current directory is already chosen; lead with the alternatives.
            return self.known.iter().filter(|d| **d != self.base).cloned().collect();
        }
        let mut scored: Vec<(u8, &PathBuf)> =
            self.known.iter().filter_map(|d| score(&q, d).map(|s| (s, d))).collect();
        scored.sort_by_key(|(s, _)| *s);
        scored.into_iter().map(|(_, d)| d.clone()).collect()
    }

    fn children(&self) -> Vec<PathBuf> {
        let (dir, stem) = match self.query.rfind('/') {
            Some(i) => (&self.query[..=i], &self.query[i + 1..]),
            None => ("", self.query.as_str()),
        };
        let parent = self.abs(if dir.is_empty() { "." } else { dir });
        let mut out = Vec::new();
        // "~/dev/" or "..": the directory itself comes first so Enter takes it.
        let whole = self.abs(&self.query);
        // ("" is all dots too.)
        let bare = stem.chars().all(|c| c == '.') || self.query == "~";
        if bare && whole.is_dir() {
            out.push(std::fs::canonicalize(&whole).unwrap_or(whole));
            if !stem.is_empty() {
                return out;
            }
        }
        let Ok(rd) = std::fs::read_dir(&parent) else { return out };
        let stem_l = stem.to_lowercase();
        let mut found: Vec<(u8, String)> = rd
            .flatten()
            .filter(|e| e.path().is_dir())
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| stem.starts_with('.') || !n.starts_with('.'))
            .filter_map(|n| {
                let l = n.to_lowercase();
                let s = if n.starts_with(stem) {
                    0
                } else if l.starts_with(&stem_l) {
                    1
                } else if l.contains(&stem_l) {
                    2
                } else {
                    return None;
                };
                Some((s, n))
            })
            .collect();
        found.sort();
        out.extend(found.into_iter().map(|(_, n)| parent.join(n)));
        out
    }
}

fn looks_like_path(q: &str) -> bool {
    q.starts_with(['/', '~', '.']) || q.contains('/')
}

/// Lower is better; None means no match. Ties keep recency order.
fn score(q: &str, d: &Path) -> Option<u8> {
    let name = d.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    let full = data::tilde(d).to_lowercase();
    if name == q {
        Some(0)
    } else if name.starts_with(q) {
        Some(1)
    } else if name.contains(q) {
        Some(2)
    } else if full.contains(q) {
        Some(3)
    } else {
        let mut cs = full.chars();
        q.chars().all(|c| cs.any(|x| x == c)).then_some(4)
    }
}

/// Directories under $HOME, shallowest first, so you can jump to a project
/// that has no chats yet. Hidden and build directories are skipped.
pub fn home_tree() -> Vec<PathBuf> {
    const DEPTH: usize = 5;
    const CAP: usize = 5000;
    const SKIP: [&str; 4] = ["node_modules", "target", "build", "__pycache__"];
    let mut out = Vec::new();
    let mut level = vec![data::home()];
    for _ in 0..DEPTH {
        let mut next = Vec::new();
        for dir in &level {
            let Ok(rd) = std::fs::read_dir(dir) else { continue };
            let mut subs: Vec<PathBuf> = rd
                .flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .filter(|e| e.file_name().to_str().is_some_and(|n| !n.starts_with('.') && !SKIP.contains(&n)))
                .map(|e| e.path())
                .collect();
            subs.sort();
            next.extend(subs);
        }
        out.extend(next.iter().cloned());
        if out.len() >= CAP {
            out.truncate(CAP);
            break;
        }
        level = next;
    }
    out
}

/// zoxide's frecency list, if it's installed.
pub fn zoxide_dirs() -> Vec<PathBuf> {
    std::process::Command::new("zoxide")
        .args(["query", "--list"])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).lines().map(PathBuf::from).collect())
        .unwrap_or_default()
}
