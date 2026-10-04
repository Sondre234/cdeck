//! Full-text search over old transcripts, on a background thread so typing
//! in the filter never waits on disk.

use crate::data;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;

/// Queries shorter than this only filter titles and directories.
pub const MIN_QUERY: usize = 3;

pub struct Search {
    /// Query the current hits belong to (lowercased).
    pub query: String,
    /// Session id -> snippet around the first match.
    pub hits: HashMap<String, String>,
    pub running: bool,
    generation: Arc<AtomicU64>,
    tx: Sender<(u64, Option<(String, String)>)>,
    rx: Receiver<(u64, Option<(String, String)>)>,
}

impl Default for Search {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Search { query: String::new(), hits: HashMap::new(), running: false, generation: Default::default(), tx, rx }
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
        let generation = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        self.query = query.clone();
        self.hits.clear();
        self.running = false;
        if query.chars().count() < MIN_QUERY {
            return;
        }
        self.running = true;
        let current = self.generation.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let cancelled = || current.load(Ordering::Relaxed) != generation;
            for (id, file) in files {
                if cancelled() {
                    return;
                }
                if let Some(s) = data::find_in_transcript(&file, &query, cancelled) {
                    if tx.send((generation, Some((id, s)))).is_err() {
                        return;
                    }
                }
            }
            let _ = tx.send((generation, None));
        });
    }

    /// Collect results that arrived since last call; true if anything changed.
    pub fn poll(&mut self) -> bool {
        let current = self.generation.load(Ordering::Relaxed);
        let mut changed = false;
        while let Ok((generation, hit)) = self.rx.try_recv() {
            if generation != current {
                continue;
            }
            changed = true;
            match hit {
                Some((id, s)) => {
                    self.hits.insert(id, s);
                }
                None => self.running = false,
            }
        }
        changed
    }
}
