mod data;
mod dirpick;
mod theme;
mod keys;
mod live;
mod search;
mod ui;

use crossterm::event::{
    MouseButton, MouseEvent, MouseEventKind,
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers as M, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::{execute, terminal};
use data::{Entry, Running, Store};
use live::Live;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant, SystemTime};

#[derive(Clone, Copy, PartialEq)]
pub enum Mode {
    Normal,
    Insert,
    Command,
    Search,
    Space,
    Goto,
    Window,
    Compose,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Focus {
    Sidebar,
    Pane,
}

const GROUP_LIMIT: usize = 5;
/// Pseudo session id for the "+ New chat" row.
pub const NEW_CHAT: &str = "+new";

pub enum Row {
    NewChat,
    Header { cwd: PathBuf, count: usize, live: usize },
    More { hidden: usize },
    Item(Item),
}

#[derive(Clone)]
pub struct Item {
    pub id: String,
    pub cwd: PathBuf,
    pub title: String,
    pub branch: Option<String>,
    pub mtime: Option<SystemTime>,
    /// Where the filter matched inside the transcript, when it missed the title.
    pub snippet: Option<String>,
}

pub enum Status<'a> {
    Busy,
    Waiting(Option<&'a str>),
    Idle { unseen: bool },
    External { pid: u32, status: &'a str },
    Dormant,
}

pub struct App {
    pub store: Store,
    pub running: HashMap<String, Running>,
    pub lives: Vec<Live>,
    pub mode: Mode,
    pub focus: Focus,
    pub compose: String,
    pub compose_dir: PathBuf,
    /// Directory picker open on top of the new-chat screen.
    pub picker: Option<dirpick::DirPicker>,
    last_dir: Option<PathBuf>,
    pub rows: Vec<Row>,
    pub selected: Option<String>,
    pub cmdline: String,
    pub filter: String,
    pub search: search::Search,
    pub live_only: bool,
    pub msg: Option<(String, bool)>,
    pub preview: Option<(String, u64, Vec<Entry>)>,
    /// Rendered preview, plus which transcript entry each line came from
    /// (None for tool calls, which search skips).
    pub wrapped: Option<(String, u64, u16, Vec<ratatui::text::Line<'static>>, Vec<Option<usize>>)>,
    /// Chat whose preview should scroll to the search match once it's known.
    pub jump_to_match: Option<String>,
    /// Match n/N last landed on (chat, line, preview_scroll), so stepping
    /// continues from it even when the view couldn't scroll that far.
    pub match_at: Option<(String, usize, usize)>,
    pub side_offset: usize,
    pub hits: Hits,
    expanded: std::collections::HashSet<PathBuf>,
    pub preview_scroll: usize,
    pub pane: (u16, u16),
    pub help: bool,
    /// Which-key strip at the bottom; `?` toggles it and the choice is remembered.
    pub keymap: bool,
    pub tick: usize,
    confirm_resume: Option<String>,
    was_busy: HashMap<String, bool>,
    quit: bool,
}

impl App {
    fn new() -> Self {
        let mut store = Store::default();
        store.scan();
        let mut app = App {
            store,
            running: data::running(),
            lives: Vec::new(),
            mode: Mode::Normal,
            focus: Focus::Sidebar,
            compose: String::new(),
            compose_dir: PathBuf::new(),
            picker: None,
            last_dir: None,
            rows: Vec::new(),
            selected: None,
            cmdline: String::new(),
            filter: String::new(),
            search: Default::default(),
            live_only: false,
            msg: None,
            preview: None,
            wrapped: None,
            jump_to_match: None,
            match_at: None,
            side_offset: 0,
            hits: Hits::default(),
            expanded: Default::default(),
            preview_scroll: 0,
            pane: (24, 80),
            help: false,
            keymap: !keymap_hidden_flag().exists(),
            tick: 0,
            confirm_resume: None,
            was_busy: HashMap::new(),
            quit: false,
        };
        app.rebuild();
        app
    }

    pub fn live(&self, id: &str) -> Option<&Live> {
        self.lives.iter().find(|l| l.id == id)
    }

    fn live_mut(&mut self, id: &str) -> Option<&mut Live> {
        self.lives.iter_mut().find(|l| l.id == id)
    }

    pub fn status(&self, id: &str) -> Status<'_> {
        let ext = self.running.get(id);
        if let Some(l) = self.live(id) {
            // Prefer Claude's own status file; fall back to output activity.
            return match ext.map(|r| r.status.as_str()) {
                Some("busy") => Status::Busy,
                Some("waiting") => Status::Waiting(ext.and_then(|r| r.waiting_for.as_deref())),
                Some(_) => Status::Idle { unseen: l.unseen },
                None if l.busy() => Status::Busy,
                None => Status::Idle { unseen: l.unseen },
            };
        }
        match ext {
            Some(r) => Status::External { pid: r.pid, status: &r.status },
            None => Status::Dormant,
        }
    }

    pub fn selected_item(&self) -> Option<&Item> {
        let id = self.selected.as_deref()?;
        self.rows.iter().find_map(|r| match r {
            Row::Item(i) if i.id == id => Some(i),
            _ => None,
        })
    }

    pub fn selected_dir(&self) -> PathBuf {
        self.selected_item()
            .map(|i| i.cwd.clone())
            .or_else(|| self.last_dir.clone())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| data::home()))
    }

    /// Regroup sessions by directory. Groups with recent activity (or
    /// anything currently running) float to the top.
    fn rebuild(&mut self) {
        let mut items: HashMap<String, Item> = self
            .store
            .visible()
            .map(|s| {
                (
                    s.id.clone(),
                    Item {
                        id: s.id.clone(),
                        cwd: s.cwd.clone(),
                        title: s.title().into(),
                        branch: s.branch.clone(),
                        mtime: s.mtime,
                        snippet: None,
                    },
                )
            })
            .collect();
        // Fresh sessions have no transcript yet.
        for l in &self.lives {
            items.entry(l.id.clone()).or_insert_with(|| {
                let s = data::Session::new_placeholder(&l.id, &l.cwd);
                Item { id: l.id.clone(), cwd: l.cwd.clone(), title: s.title().into(), branch: None, mtime: s.mtime, snippet: None }
            });
        }
        let query = search::Query::parse(&self.filter);
        let f = query.text.to_lowercase();
        let now = SystemTime::now();
        let mut groups: HashMap<PathBuf, Vec<(SystemTime, Item)>> = HashMap::new();
        let content = (self.search.query == f).then_some(&self.search.hits);
        for mut it in items.into_values() {
            let active = self.live(&it.id).is_some() || self.running.contains_key(&it.id);
            if (self.live_only && !active) || !query.admits(&it.cwd, it.mtime) {
                continue;
            }
            if !f.is_empty()
                && !it.title.to_lowercase().contains(&f)
                && !data::tilde(&it.cwd).to_lowercase().contains(&f)
            {
                match content.and_then(|h| h.get(&it.id)) {
                    Some(s) => it.snippet = Some(s.clone()),
                    None => continue,
                }
            }
            let key = if active { now } else { it.mtime.unwrap_or(SystemTime::UNIX_EPOCH) };
            groups.entry(it.cwd.clone()).or_default().push((key, it));
        }
        let mut groups: Vec<_> = groups.into_iter().collect();
        for (_, v) in &mut groups {
            v.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.id.cmp(&b.1.id)));
        }
        groups.sort_by(|a, b| b.1[0].0.cmp(&a.1[0].0).then_with(|| a.0.cmp(&b.0)));
        self.rows.clear();
        self.rows.push(Row::NewChat);
        for (cwd, v) in groups {
            let live = v.iter().filter(|(_, i)| self.live(&i.id).is_some()).count();
            let count = v.len();
            // Big directories collapse to their most recent sessions; running
            // ones and the current selection always stay visible.
            let open = !self.filter.is_empty() || self.expanded.contains(&cwd);
            let mut shown = 0;
            let mut hidden = 0;
            let mut items = Vec::new();
            for (_, i) in v {
                let keep = open
                    || shown < GROUP_LIMIT
                    || self.live(&i.id).is_some()
                    || self.running.contains_key(&i.id)
                    || self.selected.as_deref() == Some(i.id.as_str());
                if keep {
                    shown += 1;
                    items.push(Row::Item(i));
                } else {
                    hidden += 1;
                }
            }
            self.rows.push(Row::Header { cwd: cwd.clone(), count, live });
            self.rows.extend(items);
            if hidden > 0 {
                self.rows.push(Row::More { hidden });
            } else if self.expanded.contains(&cwd) && count > GROUP_LIMIT {
                self.rows.push(Row::More { hidden: 0 });
            }
        }
        if self.selected_item().is_none() {
            let first = self.item_ids().next().map(String::from);
            self.selected = first;
        }
    }

    fn item_ids(&self) -> impl Iterator<Item = &str> {
        self.rows.iter().filter_map(|r| match r {
            Row::NewChat => Some(NEW_CHAT),
            Row::Item(i) => Some(i.id.as_str()),
            _ => None,
        })
    }

    /// Open the claude.ai-style "new chat" screen.
    fn start_compose(&mut self, dir: PathBuf) {
        self.compose_dir = std::fs::canonicalize(&dir).unwrap_or(dir);
        self.picker = None;
        self.select(Some(NEW_CHAT.into()));
        self.focus = Focus::Pane;
        self.mode = Mode::Compose;
    }

    /// Directories the picker offers: ones with chats (newest first), then zoxide's, then $HOME.
    fn known_dirs(&self) -> Vec<PathBuf> {
        let mut by_age: Vec<(SystemTime, &Path)> = self
            .store
            .visible()
            .map(|s| (s.mtime.unwrap_or(SystemTime::UNIX_EPOCH), s.cwd.as_path()))
            .chain(self.lives.iter().map(|l| (SystemTime::now(), l.cwd.as_path())))
            .collect();
        by_age.sort_by(|a, b| b.0.cmp(&a.0));
        let mut seen = std::collections::HashSet::new();
        by_age
            .into_iter()
            .map(|(_, d)| d.to_path_buf())
            .chain(dirpick::zoxide_dirs())
            .chain(dirpick::home_tree())
            .filter(|d| seen.insert(d.clone()) && d.is_dir())
            .collect()
    }

    fn open_picker(&mut self, query: &str) {
        self.picker = Some(dirpick::DirPicker::new(query, self.known_dirs(), self.compose_dir.clone()));
    }

    fn picker_key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(M::CONTROL);
        let Some(p) = self.picker.as_mut() else { return };
        match k.code {
            KeyCode::Esc => self.picker = None,
            _ if is_leave(&k) => self.picker = None,
            KeyCode::Enter => match p.resolve() {
                Some(d) => {
                    self.compose_dir = std::fs::canonicalize(&d).unwrap_or(d);
                    self.picker = None;
                }
                None => {
                    let q = p.query.clone();
                    self.error(&format!("no directory matches {q}"));
                }
            },
            KeyCode::Up => p.move_by(-1),
            KeyCode::Down => p.move_by(1),
            KeyCode::Char('p') if ctrl => p.move_by(-1),
            KeyCode::Char('n') if ctrl => p.move_by(1),
            KeyCode::Tab => p.descend(),
            KeyCode::BackTab => p.up(),
            KeyCode::Char('w') if ctrl => p.up(),
            KeyCode::Char('u') if ctrl => {
                p.query.clear();
                p.refresh();
            }
            KeyCode::Backspace => {
                p.query.pop();
                p.refresh();
            }
            KeyCode::Char(c) if !ctrl => {
                p.query.push(c);
                p.refresh();
            }
            _ => {}
        }
    }

    fn submit_compose(&mut self) {
        let prompt = std::mem::take(&mut self.compose);
        let id = data::new_uuid();
        let mut args = vec!["--session-id".into(), id.clone()];
        let prompt = prompt.trim();
        if !prompt.is_empty() {
            if prompt.starts_with('-') {
                args.push("--".into());
            }
            args.push(prompt.into());
        }
        let dir = self.compose_dir.clone();
        self.spawn(&id, &dir, args);
    }

    pub fn selected_row(&self) -> Option<usize> {
        let id = self.selected.as_deref()?;
        self.rows.iter().position(|r| match r {
            Row::NewChat => id == NEW_CHAT,
            Row::Item(i) => i.id == id,
            _ => false,
        })
    }

    fn select(&mut self, id: Option<String>) {
        if id.is_some() && id != self.selected {
            self.selected = id;
            self.preview_scroll = 0;
            self.jump_to_match = self.selected.clone();
            self.confirm_resume = None;
            if let Some(cwd) = self.selected_item().map(|i| i.cwd.clone()) {
                self.last_dir = Some(cwd);
            }
            // Looking at it counts as seeing it.
            if let Some(l) = self.selected.clone().and_then(|id| self.live_mut(&id)) {
                l.unseen = false;
            }
        }
    }

    fn move_by(&mut self, d: isize) {
        let ids: Vec<String> = self.item_ids().map(String::from).collect();
        if ids.is_empty() {
            return;
        }
        let cur = self.selected.as_ref().and_then(|s| ids.iter().position(|i| i == s)).unwrap_or(0) as isize;
        let n = (cur + d).clamp(0, ids.len() as isize - 1) as usize;
        self.select(Some(ids[n].clone()));
    }

    /// Jump to the first session of the next/previous directory group.
    fn jump_group(&mut self, forward: bool) {
        let Some(cur) = self.selected_row() else { return };
        let headers: Vec<usize> =
            self.rows.iter().enumerate().filter(|(_, r)| matches!(r, Row::Header { .. })).map(|(i, _)| i).collect();
        let mine = headers.iter().rev().find(|&&h| h < cur).copied().unwrap_or(0);
        let target = if forward {
            headers.iter().find(|&&h| h > cur).copied()
        } else {
            headers.iter().rev().find(|&&h| h < mine).copied().or((cur > mine + 1).then_some(mine))
        };
        if let Some(Row::Item(i)) = target.and_then(|t| self.rows.get(t + 1)) {
            self.select(Some(i.id.clone()));
        }
    }

    /// Tab: cycle through sessions running inside cdeck, attention-needing first.
    fn cycle_live(&mut self, forward: bool) {
        let ids: Vec<String> = self.item_ids().filter(|i| self.live(i).is_some()).map(String::from).collect();
        if ids.is_empty() {
            return self.info("no live sessions — o starts one");
        }
        let pos = self.selected.as_ref().and_then(|s| ids.iter().position(|i| i == s));
        let n = ids.len();
        let next = match (pos, forward) {
            (Some(p), true) => (p + 1) % n,
            (Some(p), false) => (p + n - 1) % n,
            (None, _) => 0,
        };
        self.select(Some(ids[next].clone()));
    }

    fn info(&mut self, s: &str) {
        self.msg = Some((s.into(), false));
    }

    fn error(&mut self, s: &str) {
        self.msg = Some((s.into(), true));
    }

    fn spawn(&mut self, id: &str, cwd: &Path, args: Vec<String>) {
        if !cwd.is_dir() {
            return self.error(&format!("{} does not exist", data::tilde(cwd)));
        }
        match Live::spawn(id, cwd, &args, self.pane) {
            Ok(l) => {
                self.lives.push(l);
                self.rebuild();
                self.select(Some(id.into()));
                self.mode = Mode::Insert;
                self.focus = Focus::Pane;
                self.msg = None;
            }
            Err(e) => self.error(&format!("spawn failed: {e}")),
        }
    }

    fn new_in(&mut self, dir: PathBuf) {
        let id = data::new_uuid();
        self.spawn(&id, &dir, vec!["--session-id".into(), id.clone()]);
    }

    fn open(&mut self, force: bool) {
        if self.selected.as_deref() == Some(NEW_CHAT) {
            return self.start_compose(self.selected_dir());
        }
        let Some(item) = self.selected_item().cloned() else { return };
        if let Some(l) = self.live_mut(&item.id) {
            l.reset_scroll();
            l.unseen = false;
            self.mode = Mode::Insert;
            self.focus = Focus::Pane;
            return;
        }
        if let Some(r) = self.running.get(&item.id) {
            if !force && self.confirm_resume.as_deref() != Some(item.id.as_str()) {
                let pid = r.pid;
                self.confirm_resume = Some(item.id.clone());
                return self.error(&format!("already running elsewhere (pid {pid}) — Enter again to resume a second copy"));
            }
        }
        self.spawn(&item.id, &item.cwd, vec!["--resume".into(), item.id.clone()]);
    }

    fn kill_selected(&mut self) {
        let Some(id) = self.selected.clone() else { return };
        if let Some(pos) = self.lives.iter().position(|l| l.id == id) {
            self.lives.remove(pos);
            self.info("killed — transcript kept, Enter resumes it");
            self.rebuild();
        } else {
            self.error("not running in cdeck");
        }
    }

    fn scroll(&mut self, up: bool, amount: usize) {
        self.jump_to_match = None;
        let id = self.selected.clone().unwrap_or_default();
        if let Some(l) = self.live(&id) {
            l.scroll(if up { amount as isize } else { -(amount as isize) });
        } else if up {
            self.preview_scroll += amount;
        } else {
            self.preview_scroll = self.preview_scroll.saturating_sub(amount);
        }
    }

    /// The selected chat's transcript preview has search matches to step through.
    pub fn has_matches(&self) -> bool {
        self.selected.as_deref().is_some_and(|id| self.live(id).is_none() && self.search.hits.contains_key(id))
    }

    /// n/N: step through search matches in a transcript preview, wrapping.
    /// False when the chat has no hits, so the key keeps its usual meaning.
    fn step_match(&mut self, forward: bool) -> bool {
        if !self.has_matches() {
            return false;
        }
        let id = self.selected.clone().unwrap_or_default();
        let Some((_, _, _, lines, owners)) = self.wrapped.as_ref().filter(|w| w.0 == id) else { return false };
        let (len, ms) = (lines.len(), ui::match_lines(lines, owners, &self.search.query));
        if ms.is_empty() {
            self.info("the match is split across lines; scroll to find it");
            return true;
        }
        let h = self.pane.0 as usize;
        // From the last match we landed on, else from just above the view.
        let anchor = match &self.match_at {
            Some((mid, line, scroll)) if *mid == id && *scroll == self.preview_scroll => *line,
            _ => len.saturating_sub(self.preview_scroll + h) + 1,
        };
        let i = if forward {
            ms.iter().position(|&m| m > anchor).unwrap_or(0)
        } else {
            ms.iter().rposition(|&m| m < anchor).unwrap_or(ms.len() - 1)
        };
        self.preview_scroll = len.saturating_sub(ms[i].saturating_sub(2) + h).min(len.saturating_sub(h));
        self.jump_to_match = None;
        self.match_at = Some((id, ms[i], self.preview_scroll));
        self.info(&format!("match {}/{}", i + 1, ms.len()));
        true
    }

    fn run_command(&mut self) {
        let line = std::mem::take(&mut self.cmdline);
        let (cmd, arg) = line.trim().split_once(' ').map(|(c, a)| (c, a.trim())).unwrap_or((line.trim(), ""));
        match cmd {
            "" => {}
            "q" | "quit" | "qa" if !self.lives.is_empty() => {
                self.error(&format!("{} live session(s) — :q! kills them (transcripts are kept)", self.lives.len()))
            }
            "q" | "quit" | "qa" | "q!" | "quit!" | "qa!" => self.quit = true,
            // Anything that isn't a directory becomes the picker's query.
            "n" | "new" => {
                let dir = data::expand_tilde(arg);
                if !arg.is_empty() && dir.is_dir() {
                    self.start_compose(dir);
                } else {
                    self.start_compose(self.selected_dir());
                    if !arg.is_empty() {
                        self.open_picker(arg);
                    }
                }
            }
            "o" | "open" => {
                let dir = if arg.is_empty() { self.selected_dir() } else { data::expand_tilde(arg) };
                let dir = if dir.is_dir() {
                    Some(dir)
                } else {
                    dirpick::DirPicker::new(arg, self.known_dirs(), self.selected_dir()).resolve()
                };
                match dir {
                    Some(d) => self.new_in(std::fs::canonicalize(&d).unwrap_or(d)),
                    None => self.error(&format!("no directory matches {arg}")),
                }
            }
            "k" | "kill" => self.kill_selected(),
            "resume" | "resume!" => self.open(cmd.ends_with('!')),
            "r" | "refresh" => self.refresh(),
            "live" => {
                self.live_only = !self.live_only;
                self.rebuild();
            }
            "h" | "help" => self.help = true,
            _ => self.error(&format!("unknown command: {cmd}")),
        }
    }

    /// z: show every session in the selected directory, or fold it back.
    fn toggle_group(&mut self) {
        let Some(cwd) = self.selected_item().map(|i| i.cwd.clone()) else { return };
        if !self.expanded.remove(&cwd) {
            self.expanded.insert(cwd.clone());
        } else {
            // Folding may hide the selection; land on the group's newest session.
            self.selected = None;
            self.rebuild();
            let first = self.rows.iter().find_map(|r| match r {
                Row::Item(i) if i.cwd == cwd => Some(i.id.clone()),
                _ => None,
            });
            self.select(first);
        }
        self.rebuild();
    }

    fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Sidebar => Focus::Pane,
            Focus::Pane => Focus::Sidebar,
        };
    }

    fn refresh(&mut self) {
        self.store.scan();
        self.running = data::running();
        self.rebuild();
    }

    /// Results land in the chat list, so that's where focus goes.
    fn start_search(&mut self) {
        self.mode = Mode::Search;
        self.focus = Focus::Sidebar;
    }

    /// Transcripts to search (id, file, mtime), newest first.
    fn search_files(&self) -> Vec<(String, PathBuf, Option<SystemTime>)> {
        let mut files: Vec<_> = self.store.visible().map(|s| (s.id.clone(), s.file.clone(), s.mtime)).collect();
        files.sort_by(|a, b| b.2.cmp(&a.2));
        files
    }

    /// Kick off a transcript search for the current filter.
    fn update_search(&mut self) {
        let files = self.search_files().into_iter().map(|(id, f, _)| (id, f)).collect();
        self.search.start(&search::Query::parse(&self.filter).text, files);
        self.jump_to_match = self.selected.clone();
    }

    fn complete_path(&mut self) {
        let Some(arg) = ["new ", "n ", "open ", "o "].iter().find_map(|p| self.cmdline.strip_prefix(p)) else { return };
        let prefix_len = self.cmdline.len() - arg.len();
        let (dir, stem) = match arg.rfind('/') {
            Some(i) => (&arg[..=i], &arg[i + 1..]),
            None => ("", arg),
        };
        let base = if dir.is_empty() { PathBuf::from(".") } else { data::expand_tilde(dir) };
        let Ok(rd) = std::fs::read_dir(base) else { return };
        let mut names: Vec<String> = rd
            .flatten()
            .filter(|e| e.path().is_dir())
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| n.starts_with(stem) && (stem.starts_with('.') || !n.starts_with('.')))
            .collect();
        names.sort();
        let Some(first) = names.first() else { return };
        let common = names.iter().fold(first.clone(), |acc, n| {
            acc.chars().zip(n.chars()).take_while(|(a, b)| a == b).map(|(a, _)| a).collect()
        });
        let suffix = if names.len() == 1 { "/" } else { "" };
        self.cmdline = format!("{}{}{}{}", &self.cmdline[..prefix_len], dir, common, suffix);
        if names.len() > 1 {
            self.info(&names.join("  "));
        }
    }

    fn on_key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(M::CONTROL);
        if self.help {
            self.help = false;
            return;
        }
        match self.mode {
            Mode::Compose if self.picker.is_some() => self.picker_key(k),
            Mode::Compose => match k.code {
                KeyCode::Esc => {
                    self.mode = Mode::Normal;
                    self.focus = Focus::Sidebar;
                }
                _ if is_leave(&k) => {
                    self.mode = Mode::Normal;
                    self.focus = Focus::Sidebar;
                }
                KeyCode::Enter if k.modifiers.intersects(M::SHIFT | M::ALT) => self.compose.push('\n'),
                KeyCode::Enter => self.submit_compose(),
                KeyCode::Tab | KeyCode::BackTab => self.open_picker(""),
                KeyCode::Backspace => {
                    self.compose.pop();
                }
                KeyCode::Char('u') if ctrl => self.compose.clear(),
                KeyCode::Char('w') if ctrl => {
                    let t = self.compose.trim_end();
                    let cut = t.rfind(char::is_whitespace).map(|i| i + 1).unwrap_or(0);
                    self.compose.truncate(cut);
                }
                KeyCode::Char('j') if ctrl => self.compose.push('\n'),
                KeyCode::Char(c) if !ctrl => self.compose.push(c),
                _ => {}
            },
            Mode::Insert => {
                // Back to the chat list, ready to pick another chat.
                if is_leave(&k) {
                    self.mode = Mode::Normal;
                    self.focus = Focus::Sidebar;
                    return;
                }
                let id = self.selected.clone().unwrap_or_default();
                match self.live(&id) {
                    Some(l) => {
                        l.reset_scroll();
                        let app_cursor = l.parser.lock().unwrap().screen().application_cursor();
                        l.write(&keys::encode(k, app_cursor));
                    }
                    None => self.mode = Mode::Normal,
                }
            }
            Mode::Command => match k.code {
                KeyCode::Esc => {
                    self.cmdline.clear();
                    self.mode = Mode::Normal;
                }
                KeyCode::Enter => {
                    self.mode = Mode::Normal;
                    self.run_command();
                }
                KeyCode::Tab => self.complete_path(),
                KeyCode::Backspace if self.cmdline.is_empty() => self.mode = Mode::Normal,
                KeyCode::Backspace => {
                    self.cmdline.pop();
                }
                KeyCode::Char('u') if ctrl => self.cmdline.clear(),
                KeyCode::Char('w') if ctrl => {
                    let t = self.cmdline.trim_end_matches('/');
                    let cut = t.rfind([' ', '/']).map(|i| i + 1).unwrap_or(0);
                    self.cmdline.truncate(cut);
                }
                KeyCode::Char(c) => self.cmdline.push(c),
                _ => {}
            },
            Mode::Search => {
                match k.code {
                    KeyCode::Esc => {
                        self.filter.clear();
                        self.mode = Mode::Normal;
                    }
                    KeyCode::Enter => self.mode = Mode::Normal,
                    KeyCode::Backspace if self.filter.is_empty() => self.mode = Mode::Normal,
                    KeyCode::Backspace => {
                        self.filter.pop();
                    }
                    KeyCode::Char('u') if ctrl => self.filter.clear(),
                    KeyCode::Char(c) => self.filter.push(c),
                    _ => {}
                }
                self.update_search();
                self.rebuild();
            }
            Mode::Space => {
                self.mode = Mode::Normal;
                match k.code {
                    KeyCode::Char('f') | KeyCode::Char('/') => self.start_search(),
                    KeyCode::Char('n') => self.start_compose(self.selected_dir()),
                    KeyCode::Char('o') => self.new_in(self.selected_dir()),
                    KeyCode::Char('k') => self.kill_selected(),
                    KeyCode::Char('l') => {
                        self.live_only = !self.live_only;
                        self.rebuild();
                    }
                    KeyCode::Char('r') => self.refresh(),
                    KeyCode::Char('q') => {
                        self.cmdline = "q".into();
                        self.run_command();
                    }
                    KeyCode::Char('?') => self.help = true,
                    _ => {}
                }
            }
            Mode::Goto => {
                self.mode = Mode::Normal;
                match k.code {
                    KeyCode::Char('g') => self.move_by(isize::MIN / 2),
                    KeyCode::Char('e') => self.move_by(isize::MAX / 2),
                    KeyCode::Char('n') => self.jump_group(true),
                    KeyCode::Char('p') => self.jump_group(false),
                    KeyCode::Left | KeyCode::Char('h') => self.focus = Focus::Sidebar,
                    KeyCode::Right | KeyCode::Char('l') => self.focus = Focus::Pane,
                    _ => {}
                }
            }
            // Ctrl-w prefix, like helix window mode.
            Mode::Window => {
                self.mode = Mode::Normal;
                match k.code {
                    KeyCode::Left | KeyCode::Char('h') => self.focus = Focus::Sidebar,
                    KeyCode::Right | KeyCode::Char('l') => self.focus = Focus::Pane,
                    KeyCode::Char('w') => self.toggle_focus(),
                    _ => {}
                }
            }
            Mode::Normal => {
                self.msg = None;
                let half = (self.pane.0 / 2).max(1) as usize;
                // Focus changes work from either side.
                match k.code {
                    KeyCode::Left if ctrl => return self.focus = Focus::Sidebar,
                    KeyCode::Right if ctrl => return self.focus = Focus::Pane,
                    KeyCode::Char('w') if ctrl => return self.mode = Mode::Window,
                    KeyCode::Char('n') if ctrl => return self.start_compose(self.selected_dir()),
                    _ => {}
                }
                if self.focus == Focus::Pane {
                    let shift = k.modifiers.intersects(M::SHIFT | M::CONTROL);
                    let step = if shift { half } else { 3 };
                    match k.code {
                        KeyCode::Up | KeyCode::Char('k') => return self.scroll(true, step),
                        KeyCode::Down | KeyCode::Char('j') => return self.scroll(false, step),
                        KeyCode::Home => return self.scroll(true, usize::MAX / 4),
                        KeyCode::End => return self.scroll(false, usize::MAX / 4),
                        KeyCode::Left | KeyCode::Esc => return self.focus = Focus::Sidebar,
                        KeyCode::Char('n') if self.step_match(true) => return,
                        KeyCode::Char('N') if self.step_match(false) => return,
                        _ => {}
                    }
                }
                match k.code {
                    KeyCode::Char('d') if ctrl => self.scroll(false, half),
                    KeyCode::Char('u') if ctrl => self.scroll(true, half),
                    KeyCode::Char('j') | KeyCode::Down => self.move_by(1),
                    KeyCode::Char('k') | KeyCode::Up => self.move_by(-1),
                    KeyCode::Char('g') => self.mode = Mode::Goto,
                    KeyCode::Char('G') | KeyCode::End => self.move_by(isize::MAX / 2),
                    KeyCode::Home => self.move_by(isize::MIN / 2),
                    KeyCode::Char(']') | KeyCode::Right => self.jump_group(true),
                    KeyCode::Char('[') | KeyCode::Left => self.jump_group(false),
                    KeyCode::PageDown => self.scroll(false, half * 2),
                    KeyCode::PageUp => self.scroll(true, half * 2),
                    KeyCode::Enter | KeyCode::Char('l') | KeyCode::Char('i') | KeyCode::Char('a') => {
                        self.open(false)
                    }
                    KeyCode::Char('o') => self.new_in(self.selected_dir()),
                    KeyCode::Char('n') => self.start_compose(self.selected_dir()),
                    KeyCode::Char('O') => {
                        self.start_compose(self.selected_dir());
                        self.open_picker("");
                    }
                    KeyCode::Char('d') => self.kill_selected(),
                    KeyCode::Char('/') => self.start_search(),
                    KeyCode::Char(':') => self.mode = Mode::Command,
                    KeyCode::Char(' ') => self.mode = Mode::Space,
                    KeyCode::Char('?') => self.toggle_keymap(),
                    KeyCode::Char('r') => self.refresh(),
                    KeyCode::Char('z') => self.toggle_group(),
                    KeyCode::Tab => self.cycle_live(true),
                    KeyCode::BackTab => self.cycle_live(false),
                    KeyCode::Esc => {
                        if !self.filter.is_empty() {
                            self.filter.clear();
                            self.update_search();
                            self.rebuild();
                        }
                    }
                    KeyCode::Char('q') => self.info("helix style: :q to quit"),
                    _ => {}
                }
            }
        }
    }

    fn on_paste(&mut self, s: &str) {
        match self.mode {
            Mode::Insert => {
                if let Some(l) = self.selected.as_deref().and_then(|id| self.live(id)) {
                    l.paste(s);
                }
            }
            Mode::Compose => match self.picker.as_mut() {
                Some(p) => {
                    p.query.push_str(s.lines().next().unwrap_or(""));
                    p.refresh();
                }
                None => self.compose.push_str(s),
            },
            Mode::Command => self.cmdline.push_str(s.lines().next().unwrap_or("")),
            Mode::Search => {
                self.filter.push_str(s.lines().next().unwrap_or(""));
                self.update_search();
                self.rebuild();
            }
            _ => {}
        }
    }

    /// Reap dead children and notice sessions that finished in the background.
    fn housekeep(&mut self) {
        let before = self.lives.len();
        self.lives.retain_mut(|l| !l.reap());
        if self.lives.len() != before {
            if self.mode == Mode::Insert && self.selected.as_deref().is_some_and(|id| self.live(id).is_none()) {
                self.mode = Mode::Normal;
            }
            self.info("session exited");
            self.rebuild();
        }
        let sel = self.selected.clone();
        let ids: Vec<String> = self.lives.iter().map(|l| l.id.clone()).collect();
        for id in ids {
            let busy = matches!(self.status(&id), Status::Busy);
            let was = self.was_busy.insert(id.clone(), busy).unwrap_or(false);
            let watching = sel.as_deref() == Some(id.as_str());
            if was && !busy && !watching {
                if let Some(l) = self.live_mut(&id) {
                    l.unseen = true;
                }
            }
        }
    }
}

fn main() -> std::io::Result<()> {
    let mut app = App::new();
    let mut term = ratatui::init();
    let mut out = std::io::stdout();
    let enhanced = terminal::supports_keyboard_enhancement().unwrap_or(false);
    if enhanced {
        let _ = execute!(out, PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES));
    }
    let _ = execute!(out, event::EnableBracketedPaste, event::EnableMouseCapture);

    let mut last_gen = u64::MAX;
    let mut last_draw = Instant::now() - Duration::from_secs(1);
    let mut last_poll = Instant::now();
    let mut last_scan = Instant::now();
    let mut dirty = true;
    while !app.quit {
        if last_poll.elapsed() >= Duration::from_millis(500) {
            last_poll = Instant::now();
            app.running = data::running();
            app.housekeep();
            // Switching Claude Code themes recolours cdeck live.
            if theme::reload_if_changed() {
                app.wrapped = None;
            }
            if last_scan.elapsed() >= Duration::from_secs(3) {
                last_scan = Instant::now();
                app.store.scan();
                // Chats that grew may match now.
                let files = app.search_files();
                app.search.rescan(files);
            }
            app.rebuild();
            dirty = true;
        }
        if app.search.poll() {
            app.rebuild();
            dirty = true;
        }
        let size = term.size()?;
        let pane = ui::pane_size(size.into(), app.keymap);
        app.pane = pane;
        for l in &mut app.lives {
            l.resize(pane);
        }
        let generation = live::GENERATION.load(Ordering::Relaxed);
        let animate = last_draw.elapsed() >= Duration::from_millis(120);
        if dirty || generation != last_gen || animate {
            if animate {
                app.tick += 1;
            }
            term.draw(|f| ui::draw(f, &mut app))?;
            last_gen = generation;
            last_draw = Instant::now();
            dirty = false;
        }
        if event::poll(Duration::from_millis(16))? {
            // Drain everything queued so a fast typist doesn't wait on redraws.
            loop {
                match event::read()? {
                    Event::Key(k) if k.kind != KeyEventKind::Release => app.on_key(k),
                    Event::Paste(s) => app.on_paste(&s),
                    Event::Mouse(m) => app.on_mouse(m),
                    _ => {}
                }
                if !event::poll(Duration::ZERO)? {
                    break;
                }
            }
            dirty = true;
        }
    }
    app.lives.clear();
    if enhanced {
        let _ = execute!(out, PopKeyboardEnhancementFlags);
    }
    let _ = execute!(out, event::DisableBracketedPaste, event::DisableMouseCapture);
    ratatui::restore();
    Ok(())
}

/// Ctrl-\ stops typing into claude. Claude Code never binds it (it's the
/// terminal's SIGQUIT key, which raw mode turns off), so it can't clash the
/// way Ctrl-] (claude's "open artifact") did. Legacy terminals report it as Ctrl-4.
fn is_leave(k: &KeyEvent) -> bool {
    k.modifiers.contains(M::CONTROL) && matches!(k.code, KeyCode::Char('\\') | KeyCode::Char('4'))
}

fn keymap_hidden_flag() -> PathBuf {
    let state = std::env::var_os("XDG_STATE_HOME").map(PathBuf::from).unwrap_or_else(|| data::home().join(".local/state"));
    state.join("cdeck/keymap-hidden")
}

impl App {
    fn toggle_keymap(&mut self) {
        self.keymap = !self.keymap;
        let flag = keymap_hidden_flag();
        if self.keymap {
            let _ = std::fs::remove_file(&flag);
            self.info("key map on · ? hides it");
        } else {
            let _ = flag.parent().map(std::fs::create_dir_all);
            let _ = std::fs::write(&flag, "");
            self.info("key map hidden · ? brings it back · space ? for full help");
        }
    }
}

/// Where things were drawn last frame, so mouse events can be mapped back.
#[derive(Default)]
pub struct Hits {
    pub side: ratatui::layout::Rect,
    pub list: ratatui::layout::Rect,
    /// Row index for each visible line of the chat list (None = spacer).
    pub rows: Vec<Option<usize>>,
    pub pane: ratatui::layout::Rect,
}

impl App {
    fn on_mouse(&mut self, m: MouseEvent) {
        let at = ratatui::layout::Position::new(m.column, m.row);
        let in_side = self.hits.side.contains(at);
        let in_pane = self.hits.pane.contains(at);
        if self.help {
            if matches!(m.kind, MouseEventKind::Down(_)) {
                self.help = false;
            }
            return;
        }
        match m.kind {
            // Wheel over the chat list walks the chats; over the pane it scrolls.
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp if in_side => {
                self.leave_pane_modes();
                self.move_by(if m.kind == MouseEventKind::ScrollDown { 1 } else { -1 });
            }
            MouseEventKind::ScrollUp if in_pane => self.scroll(true, 3),
            MouseEventKind::ScrollDown if in_pane => self.scroll(false, 3),
            MouseEventKind::Down(MouseButton::Left) if self.hits.list.contains(at) => {
                let line = (m.row - self.hits.list.y) as usize;
                let Some(Some(i)) = self.hits.rows.get(line).copied() else { return };
                self.leave_pane_modes();
                self.click_row(i);
            }
            MouseEventKind::Down(MouseButton::Left) if in_pane => {
                if !matches!(self.mode, Mode::Normal | Mode::Insert | Mode::Compose) {
                    return;
                }
                if self.mode == Mode::Normal {
                    // Clicking into a chat is like pressing Enter on it.
                    match self.selected.as_deref() {
                        Some(id) if id == NEW_CHAT || self.live(id).is_some() => self.open(false),
                        _ => self.focus = Focus::Pane,
                    }
                }
            }
            _ => {}
        }
    }

    /// Typing/compose belong to the pane; touching the list takes you out of them.
    fn leave_pane_modes(&mut self) {
        if matches!(self.mode, Mode::Insert | Mode::Compose) {
            self.mode = Mode::Normal;
        }
        if self.mode == Mode::Normal {
            self.focus = Focus::Sidebar;
        }
    }

    /// Click selects; clicking the selected chat again opens it.
    fn click_row(&mut self, i: usize) {
        let id = match &self.rows[i] {
            Row::NewChat => NEW_CHAT.to_string(),
            Row::Item(it) => it.id.clone(),
            Row::Header { .. } => match self.rows.get(i + 1) {
                Some(Row::Item(it)) => it.id.clone(),
                _ => return,
            },
            Row::More { .. } => {
                let first = self.rows[..i].iter().rev().find_map(|r| match r {
                    Row::Item(it) => Some(it.id.clone()),
                    _ => None,
                });
                self.select(first);
                return self.toggle_group();
            }
        };
        if self.selected.as_deref() == Some(id.as_str()) && matches!(self.rows[i], Row::NewChat | Row::Item(_)) {
            self.open(false);
        } else {
            self.select(Some(id));
        }
    }
}
