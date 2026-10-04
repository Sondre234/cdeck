//! Full-text search over old transcripts, on a background thread so typing
//! in the filter never waits on disk.

use crate::data;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::SystemTime;

/// One transcript searched (id, mtime it had, snippet if it matched), or the end of a pass.
type Msg = (u64, Option<(String, Option<SystemTime>, Option<String>)>);

/// Queries shorter than this only filter titles and directories.
pub const MIN_QUERY: usize = 3;

pub struct Search {
    /// Query the current hits belong to (lowercased).
    pub query: String,
    /// Session id -> snippet around the first match.
    pub hits: HashMap<String, String>,
    pub running: bool,
    /// Session id -> mtime when last searched for `query`, so rescans only
    /// look at transcripts that changed since.
    searched: HashMap<String, Option<SystemTime>>,
    /// A rescan pass is in flight (quietly: no "searching…" in the footer).
    rescanning: bool,
    generation: Arc<AtomicU64>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
}

impl Default for Search {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Search {
            query: String::new(),
            hits: HashMap::new(),
            running: false,
            searched: HashMap::new(),
            rescanning: false,
            generation: Default::default(),
            tx,
            rx,
        }
    }
}

impl Search {
    /// Start searching `files` (id, path), newest first, for `query`.
    /// Any search already in flight is abandoned.
    pub fn start(&mut self, query: &str, files: Vec<(String, PathBuf)>) {
        let query = query.to_lowercase();
        if query == self.query {
            return;
        }
        self.generation.fetch_add(1, Ordering::Relaxed);
        self.query = query;
        self.hits.clear();
        self.searched.clear();
        self.running = false;
        self.rescanning = false;
        if self.query.chars().count() < MIN_QUERY {
            return;
        }
        self.running = true;
        self.spawn(files);
    }

    /// Re-search transcripts that changed (or appeared) since they were last
    /// searched for the current query. Chats that already matched keep their
    /// snippet: transcripts only grow, so the first match stays the first.
    pub fn rescan(&mut self, files: Vec<(String, PathBuf, Option<SystemTime>)>) {
        if self.running || self.rescanning || self.query.chars().count() < MIN_QUERY {
            return;
        }
        let stale: Vec<_> = files
            .into_iter()
            .filter(|(id, _, mtime)| !self.hits.contains_key(id) && self.searched.get(id) != Some(mtime))
            .map(|(id, file, _)| (id, file))
            .collect();
        if !stale.is_empty() {
            self.rescanning = true;
            self.spawn(stale);
        }
    }

    fn spawn(&self, files: Vec<(String, PathBuf)>) {
        let generation = self.generation.load(Ordering::Relaxed);
        let current = self.generation.clone();
        let tx = self.tx.clone();
        let query = self.query.clone();
        std::thread::spawn(move || {
            let cancelled = || current.load(Ordering::Relaxed) != generation;
            for (id, file) in files {
                if cancelled() {
                    return;
                }
                // Stat before reading: if it grows mid-search, the next rescan catches it.
                let mtime = std::fs::metadata(&file).and_then(|m| m.modified()).ok();
                let hit = data::find_in_transcript(&file, &query, cancelled);
                if cancelled() || tx.send((generation, Some((id, mtime, hit)))).is_err() {
                    return;
                }
            }
            let _ = tx.send((generation, None));
        });
    }

    /// Collect results that arrived since last call; true if anything visible changed.
    pub fn poll(&mut self) -> bool {
        let current = self.generation.load(Ordering::Relaxed);
        let mut changed = false;
        while let Ok((generation, msg)) = self.rx.try_recv() {
            if generation != current {
                continue;
            }
            match msg {
                Some((id, mtime, hit)) => {
                    self.searched.insert(id.clone(), mtime);
                    if let Some(s) = hit {
                        self.hits.insert(id, s);
                        changed = true;
                    }
                }
                None => {
                    changed |= self.running;
                    self.running = false;
                    self.rescanning = false;
                }
            }
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn settle(s: &mut Search) {
        let t = std::time::Instant::now();
        while (s.running || s.rescanning) && t.elapsed().as_secs() < 5 {
            s.poll();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn rescan_picks_up_matches_in_grown_transcripts() {
        let dir = std::env::temp_dir().join(format!("cdeck-search-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (a, b) = (dir.join("a.jsonl"), dir.join("b.jsonl"));
        let line = |t: &str| format!("{{\"type\":\"user\",\"message\":{{\"content\":\"{t}\"}}}}\n");
        std::fs::write(&a, line("the needle is here")).unwrap();
        std::fs::write(&b, line("nothing yet")).unwrap();
        let files = || {
            [("a", &a), ("b", &b)]
                .map(|(id, f)| (id.to_string(), f.clone(), std::fs::metadata(f).and_then(|m| m.modified()).ok()))
                .to_vec()
        };
        let mut s = Search::default();
        s.start("Needle", files().into_iter().map(|(id, f, _)| (id, f)).collect());
        settle(&mut s);
        assert!(s.hits.contains_key("a") && !s.hits.contains_key("b"));

        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::OpenOptions::new().append(true).open(&b).unwrap().write_all(line("now a needle").as_bytes()).unwrap();
        s.rescan(files());
        assert!(!s.running, "rescans stay quiet");
        settle(&mut s);
        assert!(s.hits.contains_key("a") && s.hits.contains_key("b"));
        // Nothing changed since: nothing to do.
        s.rescan(files());
        assert!(!s.rescanning);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
