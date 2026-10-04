mod config;
mod data;
mod dirpick;
mod theme;
mod keys;
mod live;
mod search;
mod ui;
mod window;

use crossterm::event::{
    MouseButton, MouseEvent, MouseEventKind,
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers as M, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::{execute, terminal};
use data::{Entry, Running, Store};
use live::Live;
use std::collections::{HashMap, HashSet};
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

/// Pseudo session id for the "+ New chat" row.
pub const NEW_CHAT: &str = "+new";

pub enum Row {
    NewChat,
    /// Heads the pinned chats, which sit above the directory groups.
    Pinned,
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

pub type Wrapped = (String, u64, u16, Vec<ratatui::text::Line<'static>>, Vec<Option<usize>>);

/// Preview state of the split's right half (transcript, rendering, scroll),
/// swapped into the main fields while that half is drawn or scrolled.
pub type SplitView = (Option<(String, u64, Vec<Entry>)>, Option<Wrapped>, usize);

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
    pub wrapped: Option<Wrapped>,
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
    /// Chat held in the right half of a split pane (`Ctrl-w v`); the left
    /// half keeps following the selection.
    pub split: Option<String>,
    pub split_view: SplitView,
    /// Below `narrow_width`: list and pane take turns, and nothing splits.
    pub narrow: bool,
    /// Which-key strip at the bottom; `?` toggles it and the choice is remembered.
    pub keymap: bool,
    pub pinned: HashSet<String>,
    /// Hidden from the list; the transcripts themselves are never touched.
    pub archived: HashSet<String>,
    /// Local display names (`R`), shown instead of Claude's titles.
    pub names: HashMap<String, String>,
    pub show_archived: bool,
    pub tick: usize,
    /// Ctrl-L: throw away what's on screen and draw everything again.
    pub repaint: bool,
    confirm_resume: Option<String>,
    /// Last seen phase of each live chat, and whether Claude's status file said so.
    phase: HashMap<String, (Phase, bool)>,
    /// Desktop notifications when a chat finishes or needs you; `:notify` toggles.
    pub notify: bool,
    /// Mouse capture; off hands the mouse back to the terminal for native selection.
    pub mouse: bool,
    confirm_window: Option<String>,
    quit: bool,
}

impl App {
    fn new() -> Self {
        let mut store = Store::default();
        store.load_cache();
        store.scan();
        store.save_cache();
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
            msg: config::error().map(|e| (e.into(), true)),
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
            split: None,
            split_view: Default::default(),
            narrow: false,
            keymap: !state_file("keymap-hidden").exists(),
            pinned: load_ids("pinned"),
            archived: load_ids("archived"),
            names: parse_names(&std::fs::read_to_string(state_file("names")).unwrap_or_default()),
            show_archived: false,
            tick: 0,
            repaint: false,
            confirm_resume: None,
            phase: HashMap::new(),
            notify: !state_file("notify-off").exists(),
            mouse: !state_file("mouse-off").exists(),
            confirm_window: None,
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

    /// A local name wins over whatever Claude called the chat.
    fn title_of(&self, s: &data::Session) -> String {
        self.names.get(&s.id).cloned().unwrap_or_else(|| s.title().into())
    }

    /// A chat's list entry, or one made up for a chat the list currently
    /// hides (filtered out, say) but the split still shows.
    pub fn find_item(&self, id: &str) -> Option<Item> {
        let listed = self.rows.iter().find_map(|r| match r {
            Row::Item(i) if i.id == id => Some(i.clone()),
            _ => None,
        });
        listed.or_else(|| {
            let placeholder = self.live(id).map(|l| data::Session::new_placeholder(&l.id, &l.cwd));
            let s = self.store.sessions.get(id).or(placeholder.as_ref())?;
            Some(Item { id: s.id.clone(), cwd: s.cwd.clone(), title: self.title_of(s), branch: s.branch.clone(), mtime: s.mtime, snippet: None })
        })
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
                        title: self.title_of(s),
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
                Item { id: l.id.clone(), cwd: l.cwd.clone(), title: self.title_of(&s), branch: None, mtime: s.mtime, snippet: None }
            });
        }
        let query = search::Query::parse(&self.filter);
        let f = query.text.to_lowercase();
        let now = SystemTime::now();
        let mut groups: HashMap<PathBuf, Vec<(SystemTime, Item)>> = HashMap::new();
        // Pinned chats leave their directory group rather than showing twice.
        let mut pins: Vec<(SystemTime, Item)> = Vec::new();
        let content = (self.search.query == f).then_some(&self.search.hits);
        for mut it in items.into_values() {
            let active = self.live(&it.id).is_some() || self.running.contains_key(&it.id);
            if (self.live_only && !active) || !query.admits(&it.cwd, it.mtime) {
                continue;
            }
            // Running chats stay put until they stop, so nothing live goes missing.
            if !self.show_archived && !active && self.archived.contains(&it.id) {
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
            if self.pinned.contains(&it.id) {
                pins.push((key, it));
            } else {
                groups.entry(it.cwd.clone()).or_default().push((key, it));
            }
        }
        let newest_first = |a: &(SystemTime, Item), b: &(SystemTime, Item)| b.0.cmp(&a.0).then_with(|| a.1.id.cmp(&b.1.id));
        let mut groups: Vec<_> = groups.into_iter().collect();
        for (_, v) in &mut groups {
            v.sort_by(newest_first);
        }
        groups.sort_by(|a, b| b.1[0].0.cmp(&a.1[0].0).then_with(|| a.0.cmp(&b.0)));
        pins.sort_by(newest_first);
        self.rows.clear();
        self.rows.push(Row::NewChat);
        if !pins.is_empty() {
            self.rows.push(Row::Pinned);
            self.rows.extend(pins.into_iter().map(|(_, i)| Row::Item(i)));
        }
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
                    || shown < config::cfg().group_limit
                    || self.live(&i.id).is_some()
                    || self.running.contains_key(&i.id)
                    || self.selected.as_deref() == Some(i.id.as_str())
                    || self.split.as_deref() == Some(i.id.as_str());
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
            } else if self.expanded.contains(&cwd) && count > config::cfg().group_limit {
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
            self.confirm_window = None;
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
        let headers: Vec<usize> = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, r)| matches!(r, Row::Header { .. } | Row::Pinned))
            .map(|(i, _)| i)
            .collect();
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

    /// Branch the selected chat into a new session. Claude picks the fork's id
    /// unless told, so we choose it up front to track the new instance.
    fn fork_selected(&mut self) {
        let Some(item) = self.selected_item().cloned() else { return self.error("select a chat to fork") };
        if !self.store.sessions.contains_key(&item.id) {
            return self.error("nothing to fork yet — the chat has no transcript");
        }
        let id = data::new_uuid();
        self.spawn(&id, &item.cwd, fork_args(&item.id, &id));
        if self.live(&id).is_some() {
            self.info(&format!("forked from {}", item.title));
        }
    }

    /// The selected chat (or a fresh one in its directory) in a terminal window
    /// of its own, for when you want it next to cdeck rather than inside it.
    fn open_window(&mut self, force: bool) {
        let prog = std::env::var("CDECK_CLAUDE").unwrap_or_else(|_| "claude".into());
        let item = self.selected_item().cloned();
        let (dir, cmd) = match &item {
            Some(it) => (it.cwd.clone(), vec![prog, "--resume".into(), it.id.clone()]),
            None => (self.selected_dir(), vec![prog]),
        };
        if let Some(it) = &item {
            // Two copies of one session would both write to the same transcript.
            let here = self.live(&it.id).is_some();
            if (here || self.running.contains_key(&it.id)) && !force && self.confirm_window.as_deref() != Some(it.id.as_str()) {
                self.confirm_window = Some(it.id.clone());
                let place = if here { "in cdeck" } else { "elsewhere" };
                return self.error(&format!("already running {place} — E again opens a second copy"));
            }
        }
        if !dir.is_dir() {
            return self.error(&format!("{} does not exist", data::tilde(&dir)));
        }
        let Some(term) = window::terminal() else { return self.error("no terminal found — set $TERMINAL") };
        let name = Path::new(&term[0]).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        match window::launch(&window::command(&term, &dir, &cmd), &dir) {
            Ok(()) => self.info(&format!("opened in a new {name} window")),
            Err(e) => self.error(&format!("couldn't start {}: {e}", term[0])),
        }
    }

    fn toggle_pin(&mut self) {
        let Some(id) = self.selected_item().map(|i| i.id.clone()) else { return self.error("select a chat to pin") };
        let pinned = !self.pinned.remove(&id);
        if pinned {
            self.pinned.insert(id);
        }
        self.rebuild();
        match save_ids("pinned", &self.pinned) {
            Err(e) => self.error(&format!("couldn't save pins: {e}")),
            Ok(()) => self.info(if pinned { "pinned · p again unpins" } else { "unpinned" }),
        }
    }

    fn toggle_archive(&mut self) {
        let Some(id) = self.selected_item().map(|i| i.id.clone()) else { return self.error("select a chat to archive") };
        let archived = !self.archived.remove(&id);
        let active = self.live(&id).is_some() || self.running.contains_key(&id);
        let hides = archived && !active && !self.show_archived;
        if archived {
            self.archived.insert(id.clone());
        }
        // Step to a neighbour before it vanishes, rather than jumping to the top.
        if hides {
            let ids: Vec<&str> = self.item_ids().collect();
            let pos = ids.iter().position(|&i| i == id).unwrap_or(0);
            let next = ids.get(pos + 1).or_else(|| ids.get(pos.checked_sub(1)?)).map(|s| s.to_string());
            self.select(next);
        }
        self.rebuild();
        let msg = match (archived, hides, active) {
            (false, ..) => "unarchived",
            (true, true, _) => "archived · undo: :archived shows archived chats, then x on it",
            (true, false, true) => "archived · hidden once it stops running · x again undoes",
            (true, false, false) => "archived · x again undoes",
        };
        match save_ids("archived", &self.archived) {
            Err(e) => self.error(&format!("couldn't save archive: {e}")),
            Ok(()) => self.info(msg),
        }
    }

    /// Name the selected chat locally; an empty name goes back to Claude's title.
    fn rename_selected(&mut self, name: &str) {
        let Some(id) = self.selected_item().map(|i| i.id.clone()) else { return self.error("select a chat to rename") };
        let name: String = name.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
        let name = name.trim();
        let cleared = name.is_empty();
        if cleared {
            self.names.remove(&id);
        } else {
            self.names.insert(id, name.into());
        }
        self.rebuild();
        let path = state_file("names");
        let saved = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&path, format_names(&self.names)));
        match saved {
            Err(e) => self.error(&format!("couldn't save names: {e}")),
            Ok(()) => self.info(if cleared { "name cleared · back to Claude's title" } else { "renamed (only in cdeck) · :rename with no name undoes" }),
        }
    }

    /// R: the command line, prefilled with the current title to edit.
    fn start_rename(&mut self) {
        let Some(title) = self.selected_item().map(|i| i.title.clone()) else { return self.error("select a chat to rename") };
        self.cmdline = format!("rename {title}");
        self.mode = Mode::Command;
    }

    fn toggle_show_archived(&mut self) {
        self.show_archived = !self.show_archived;
        self.rebuild();
        self.info(if self.show_archived { "showing archived chats · x unarchives" } else { "archived chats hidden" });
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
            "fork" => self.fork_selected(),
            "pin" => self.toggle_pin(),
            "win" | "win!" => self.open_window(cmd.ends_with('!')),
            "archive" => self.toggle_archive(),
            "rename" => self.rename_selected(arg),
            "archived" => self.toggle_show_archived(),
            "resume" | "resume!" => self.open(cmd.ends_with('!')),
            "r" | "refresh" => self.refresh(),
            "live" => {
                self.live_only = !self.live_only;
                self.rebuild();
            }
            "notify" => self.toggle_notify(),
            "mouse" => self.toggle_mouse(),
            "h" | "help" => self.help = true,
            "config" => {
                let p = config::path();
                let state = if p.exists() { "" } else { " (not created yet — defaults in use)" };
                self.info(&format!("config: {}{state}", data::tilde(&p)));
            }
            _ => self.error(&format!("unknown command: {cmd}")),
        }
    }

    /// z: show every session in the selected directory, or fold it back.
    fn toggle_group(&mut self) {
        let Some(cwd) = self.selected_item().map(|i| i.cwd.clone()) else { return };
        if !self.expanded.remove(&cwd) {
            self.expanded.insert(cwd.clone());
        } else {
            // Folding may hide the selection; land on the group's newest session
            // (in the group itself, not the pinned section above it).
            self.selected = None;
            self.rebuild();
            let first = self
                .rows
                .iter()
                .skip_while(|r| !matches!(r, Row::Header { cwd: c, .. } if *c == cwd))
                .find_map(|r| match r {
                    Row::Item(i) if i.cwd == cwd => Some(i.id.clone()),
                    _ => None,
                });
            self.select(first);
        }
        self.rebuild();
    }

    /// Ctrl-w v: keep the selected chat in a right half while the left half
    /// follows the selection, so two chats can be watched side by side.
    fn split_pane(&mut self) {
        if self.narrow {
            return self.error("too narrow to split — widen the window (narrow_width in the config)");
        }
        let Some(id) = self.selected_item().map(|i| i.id.clone()) else { return self.error("select a chat to split off") };
        self.split = Some(id);
        self.info("split · the right half keeps this chat, the left follows the selection · Ctrl-w q closes");
    }

    fn unsplit(&mut self) {
        self.split = None;
        self.split_view = Default::default();
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
                    KeyCode::Char('F') => self.fork_selected(),
                    KeyCode::Char('p') => self.toggle_pin(),
                    KeyCode::Char('x') => self.toggle_archive(),
                    KeyCode::Char('a') => self.toggle_show_archived(),
                    KeyCode::Char('E') => self.open_window(false),
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
                    KeyCode::Char('v') => self.split_pane(),
                    KeyCode::Char('q') | KeyCode::Char('o') => self.unsplit(),
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
                    KeyCode::Char('l') if ctrl => return self.repaint = true,
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
                    KeyCode::Char('F') => self.fork_selected(),
                    KeyCode::Char('p') => self.toggle_pin(),
                    KeyCode::Char('x') => self.toggle_archive(),
                    KeyCode::Char('R') => self.start_rename(),
                    KeyCode::Char('E') => self.open_window(false),
                    KeyCode::Char('/') => self.start_search(),
                    KeyCode::Char(':') => self.mode = Mode::Command,
                    KeyCode::Char(' ') => self.mode = Mode::Space,
                    KeyCode::Char('?') => self.toggle_keymap(),
                    KeyCode::Char('r') => self.refresh(),
                    KeyCode::Char('z') => self.toggle_group(),
                    KeyCode::Char('M') => self.toggle_mouse(),
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
            let (now, waiting_for) = match self.status(&id) {
                Status::Busy => (Phase::Busy, None),
                Status::Waiting(w) => (Phase::Waiting, Some(w.unwrap_or("input").to_string())),
                _ => (Phase::Idle, None),
            };
            let trusted = self.running.contains_key(&id);
            let was = self.phase.insert(id.clone(), (now, trusted));
            let selected = sel.as_deref() == Some(id.as_str());
            if was.is_some_and(|w| w.0 == Phase::Busy) && now != Phase::Busy && !selected {
                if let Some(l) = self.live_mut(&id) {
                    l.unseen = true;
                }
            }
            // A selected chat only counts as watched while its pane has focus.
            // Only Claude's own status file is trusted: the output-activity
            // fallback flickers (redraws, resizes) and would spam.
            let watching = selected && self.focus == Focus::Pane;
            let was = was.filter(|w| w.1 && trusted).map(|w| w.0);
            if let Some(what) = noteworthy(was, now).filter(|_| self.notify && !watching) {
                let what = match what {
                    Phase::Waiting => format!("needs you: {}", waiting_for.unwrap_or_default()),
                    _ => "finished".into(),
                };
                self.notify_send(&id, &what);
            }
        }
        self.phase.retain(|id, _| self.lives.iter().any(|l| &l.id == id));
    }

    fn notify_send(&self, id: &str, what: &str) {
        let (title, cwd) = match self.store.sessions.get(id) {
            Some(s) => (self.title_of(s), s.cwd.clone()),
            None => (self.names.get(id).cloned().unwrap_or_else(|| "(new session)".into()), self.live(id).map(|l| l.cwd.clone()).unwrap_or_default()),
        };
        let body = format!("{} · {what}", data::tilde(&cwd));
        // Detached and best-effort: no notify-send, no notification.
        let child = std::process::Command::new("notify-send")
            .args(["-a", "cdeck", &title, &body])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        if let Ok(mut c) = child {
            std::thread::spawn(move || c.wait());
        }
    }

    fn toggle_mouse(&mut self) {
        self.mouse = !self.mouse;
        set_flag("mouse-off", !self.mouse);
        let mut out = std::io::stdout();
        if self.mouse {
            let _ = execute!(out, event::EnableMouseCapture);
            self.info("mouse on · M frees it for native text selection");
        } else {
            let _ = execute!(out, event::DisableMouseCapture);
            self.info("mouse off — select text natively, M turns it back on");
        }
    }

    fn toggle_notify(&mut self) {
        self.notify = !self.notify;
        set_flag("notify-off", !self.notify);
        self.info(if self.notify { "notifications on · :notify turns them off" } else { "notifications off · :notify turns them back on" });
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Phase {
    Idle,
    Busy,
    Waiting,
}

/// Which phase changes deserve a desktop notification: finishing a turn, and
/// starting to wait. Edge-triggered, and never on first sight, so whatever
/// was already going on when cdeck noticed a session stays quiet.
fn noteworthy(was: Option<Phase>, now: Phase) -> Option<Phase> {
    match (was?, now) {
        (a, b) if a == b => None,
        (_, Phase::Waiting) => Some(Phase::Waiting),
        (Phase::Busy, Phase::Idle) => Some(Phase::Idle),
        _ => None,
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
    let _ = execute!(out, event::EnableBracketedPaste);
    if app.mouse {
        let _ = execute!(out, event::EnableMouseCapture);
    }

    let mut last_gen = u64::MAX;
    let mut last_draw = Instant::now() - Duration::from_secs(1);
    let mut last_poll = Instant::now();
    let mut last_scan = Instant::now();
    let mut last_save = Instant::now();
    let mut dirty = true;
    let mut last_size = term.size()?;
    let mut resized_at: Option<Instant> = None;
    while !app.quit {
        if last_poll.elapsed() >= Duration::from_millis(500) {
            last_poll = Instant::now();
            app.running = data::running();
            app.housekeep();
            // Switching Claude Code themes recolours cdeck live.
            if theme::reload_if_changed() {
                app.wrapped = None;
                app.split_view.1 = None;
            }
            if last_scan.elapsed() >= Duration::from_secs(3) {
                last_scan = Instant::now();
                app.store.scan();
                // Chats that grew may match now.
                let files = app.search_files();
                app.search.rescan(files);
            }
            // Chats being written to dirty the cache constantly; don't rewrite it every scan.
            if last_save.elapsed() >= Duration::from_secs(60) {
                last_save = Instant::now();
                app.store.save_cache();
            }
            app.rebuild();
            dirty = true;
        }
        if app.search.poll() {
            app.rebuild();
            dirty = true;
        }
        let size = term.size()?;
        if size != last_size {
            last_size = size;
            resized_at = Some(Instant::now());
        }
        // A compositor animating the window (Hyprland) resizes the terminal in
        // bursts, and a frame drawn mid-burst can be partly dropped while
        // ratatui believes it's on screen. Once the size settles, repaint
        // everything rather than trusting the diff.
        if app.repaint || resized_at.is_some_and(|t| t.elapsed() >= Duration::from_millis(200)) {
            term.clear()?;
            app.repaint = false;
            resized_at = None;
            dirty = true;
        }
        app.narrow = ui::is_narrow(size.into());
        if app.narrow && app.split.is_some() {
            app.unsplit();
        }
        let pane = ui::pane_size(size.into(), app.keymap, false);
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
    app.store.save_cache();
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

/// `--session-id` is only accepted alongside `--resume` when forking.
fn fork_args(from: &str, new: &str) -> Vec<String> {
    ["--resume", from, "--fork-session", "--session-id", new].map(String::from).to_vec()
}

/// Remembered choices live as empty flag files under $XDG_STATE_HOME/cdeck.
fn state_file(name: &str) -> PathBuf {
    let state = std::env::var_os("XDG_STATE_HOME").map(PathBuf::from).unwrap_or_else(|| data::home().join(".local/state"));
    state.join("cdeck").join(name)
}

fn set_flag(name: &str, on: bool) {
    let flag = state_file(name);
    if on {
        let _ = flag.parent().map(std::fs::create_dir_all);
        let _ = std::fs::write(&flag, "");
    } else {
        let _ = std::fs::remove_file(&flag);
    }
}

/// Session ids kept one per line, so the files stay greppable and hand-editable.
fn load_ids(name: &str) -> HashSet<String> {
    std::fs::read_to_string(state_file(name)).unwrap_or_default().lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect()
}

fn save_ids(name: &str, ids: &HashSet<String>) -> std::io::Result<()> {
    let path = state_file(name);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut v: Vec<&str> = ids.iter().map(String::as_str).collect();
    v.sort();
    std::fs::write(path, v.iter().map(|id| format!("{id}\n")).collect::<String>())
}

/// Local names, one `id<TAB>name` per line: greppable and hand-editable like
/// the id lists. Lines without a tab or with an empty name are skipped.
fn parse_names(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|l| l.split_once('\t'))
        .map(|(id, name)| (id.trim().to_string(), name.trim().to_string()))
        .filter(|(id, name)| !id.is_empty() && !name.is_empty())
        .collect()
}

fn format_names(names: &HashMap<String, String>) -> String {
    let mut v: Vec<_> = names.iter().collect();
    v.sort();
    v.iter().map(|(id, name)| format!("{id}\t{name}\n")).collect()
}

impl App {
    fn toggle_keymap(&mut self) {
        self.keymap = !self.keymap;
        set_flag("keymap-hidden", !self.keymap);
        if self.keymap {
            self.info("key map on · ? hides it");
        } else {
            self.info("key map hidden · ? brings it back · space ? for full help");
        }
    }
}

/// Where things were drawn last frame, so mouse events can be mapped back.
#[derive(Default)]
pub struct Hits {
    pub side: ratatui::layout::Rect,
    /// The split's right half; empty when not split.
    pub split: ratatui::layout::Rect,
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
            Row::Header { .. } | Row::Pinned => match self.rows.get(i + 1) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notifies_on_finish_and_on_starting_to_wait_only() {
        use Phase::*;
        assert_eq!(noteworthy(None, Waiting), None, "pre-existing state stays quiet");
        assert_eq!(noteworthy(None, Idle), None);
        assert_eq!(noteworthy(Some(Busy), Idle), Some(Idle));
        assert_eq!(noteworthy(Some(Busy), Waiting), Some(Waiting));
        assert_eq!(noteworthy(Some(Idle), Waiting), Some(Waiting));
        assert_eq!(noteworthy(Some(Waiting), Waiting), None, "once per transition");
        assert_eq!(noteworthy(Some(Waiting), Busy), None);
        assert_eq!(noteworthy(Some(Waiting), Idle), None);
        assert_eq!(noteworthy(Some(Idle), Busy), None);
    }

    #[test]
    fn names_round_trip_and_skip_junk() {
        let names = parse_names("b\tSecond one\na\t  First \nno tab here\nc\t\n\n");
        assert_eq!(names.len(), 2);
        assert_eq!(names["a"], "First");
        assert_eq!(format_names(&names), "a\tFirst\nb\tSecond one\n");
        assert_eq!(parse_names(&format_names(&names)), names);
    }

    #[test]
    fn fork_names_the_new_session() {
        assert_eq!(fork_args("old", "new"), ["--resume", "old", "--fork-session", "--session-id", "new"]);
    }
}
