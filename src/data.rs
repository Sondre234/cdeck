//! Reading Claude Code's on-disk state: session transcripts under
//! ~/.claude/projects and the per-process status files under ~/.claude/sessions.

use chrono::{DateTime, Local, NaiveDate, Utc};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub fn claude_dir() -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".claude"))
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| "/".into())
}

/// `/home/me/dev/x` -> `~/dev/x`
pub fn tilde(p: &Path) -> String {
    let h = home();
    match p.strip_prefix(&h) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => p.display().to_string(),
    }
}

pub fn expand_tilde(s: &str) -> PathBuf {
    if s == "~" {
        home()
    } else if let Some(rest) = s.strip_prefix("~/") {
        home().join(rest)
    } else {
        PathBuf::from(s)
    }
}

#[derive(Clone, Default)]
pub struct Session {
    pub id: String,
    pub cwd: PathBuf,
    pub file: PathBuf,
    pub mtime: Option<SystemTime>,
    pub branch: Option<String>,
    ai_title: Option<String>,
    custom_title: Option<String>,
    first_prompt: Option<String>,
    /// Token usage per local day, oldest first, so totals and "today" both
    /// come from the cache without rereading transcripts.
    usage: Vec<(NaiveDate, Usage)>,
    /// Claude Code writes one line per content block, each repeating the
    /// message's usage; those lines are adjacent, so the last id dedupes them.
    last_msg: Option<String>,
    // Incremental parse state: transcripts are append-only.
    offset: u64,
}

/// Tokens of one or more API messages.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_write: u64,
    pub cache_read: u64,
}

impl Usage {
    /// Everything the model read: fresh, cache-written and cache-read input.
    pub fn input_total(&self) -> u64 {
        self.input + self.cache_write + self.cache_read
    }
}

impl std::ops::AddAssign for Usage {
    fn add_assign(&mut self, o: Usage) {
        self.input += o.input;
        self.output += o.output;
        self.cache_write += o.cache_write;
        self.cache_read += o.cache_read;
    }
}

impl Session {
    pub fn title(&self) -> &str {
        self.custom_title
            .as_deref()
            .or(self.ai_title.as_deref())
            .or(self.first_prompt.as_deref())
            .unwrap_or("(new session)")
    }

    pub fn new_placeholder(id: &str, cwd: &Path) -> Self {
        Session { id: id.into(), cwd: cwd.into(), mtime: Some(SystemTime::now()), ..Default::default() }
    }

    /// Tokens used over the whole chat (its own transcript; subagents keep theirs elsewhere).
    pub fn usage(&self) -> Usage {
        self.usage_since(NaiveDate::MIN)
    }

    pub fn usage_since(&self, day: NaiveDate) -> Usage {
        let mut u = Usage::default();
        for (_, d) in self.usage.iter().filter(|(d, _)| *d >= day) {
            u += *d;
        }
        u
    }

    fn has_content(&self) -> bool {
        self.first_prompt.is_some() || self.custom_title.is_some()
    }

    /// Parse whatever has been appended since the last call.
    fn update(&mut self, len: u64) {
        if len < self.offset {
            // Truncated/rewritten: start over.
            *self = Session { id: std::mem::take(&mut self.id), file: std::mem::take(&mut self.file), ..Default::default() };
        }
        let Ok(mut f) = fs::File::open(&self.file) else { return };
        if f.seek(SeekFrom::Start(self.offset)).is_err() {
            return;
        }
        let mut buf = Vec::new();
        if f.take(len - self.offset).read_to_end(&mut buf).is_err() {
            return;
        }
        // Only consume complete lines.
        let Some(end) = buf.iter().rposition(|&b| b == b'\n') else { return };
        self.offset += end as u64 + 1;
        for line in buf[..end].split(|&b| b == b'\n') {
            self.ingest(line);
        }
    }

    fn to_json(&self) -> Option<Value> {
        let ms = |t: SystemTime| t.duration_since(SystemTime::UNIX_EPOCH).ok().map(|d| d.as_millis() as u64);
        Some(json!({
            "id": self.id,
            "file": self.file.to_str()?,
            "offset": self.offset,
            "mtime": self.mtime.and_then(ms),
            "cwd": self.cwd.to_str()?,
            "branch": self.branch,
            "ai_title": self.ai_title,
            "custom_title": self.custom_title,
            "first_prompt": self.first_prompt,
            "usage": self.usage.iter().map(|(d, u)| json!([d.to_string(), u.input, u.output, u.cache_write, u.cache_read])).collect::<Vec<_>>(),
            "last_msg": self.last_msg,
        }))
    }

    fn from_json(v: &Value) -> Option<Session> {
        let s = |k: &str| v[k].as_str().map(String::from);
        Some(Session {
            id: s("id")?,
            file: s("file")?.into(),
            offset: v["offset"].as_u64()?,
            mtime: v["mtime"].as_u64().map(|ms| SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(ms)),
            cwd: s("cwd")?.into(),
            branch: s("branch"),
            ai_title: s("ai_title"),
            custom_title: s("custom_title"),
            first_prompt: s("first_prompt"),
            usage: v["usage"]
                .as_array()?
                .iter()
                .map(|e| {
                    let n = |i: usize| e[i].as_u64();
                    let day = e[0].as_str()?.parse().ok()?;
                    Some((day, Usage { input: n(1)?, output: n(2)?, cache_write: n(3)?, cache_read: n(4)? }))
                })
                .collect::<Option<_>>()?,
            last_msg: s("last_msg"),
        })
    }

    fn ingest(&mut self, line: &[u8]) {
        let Ok(line) = std::str::from_utf8(line) else { return };
        // History can be hundreds of MB, so cwd/branch are plucked out with
        // string search and only the few title/prompt lines get JSON-parsed.
        if self.cwd.as_os_str().is_empty() {
            if let Some(c) = str_field(line, "cwd") {
                self.cwd = c.into();
            }
        }
        if let Some(b) = str_field(line, "gitBranch").filter(|b| !b.is_empty() && *b != "HEAD") {
            self.branch = Some(b.into());
        }
        if let Some((id, u)) = message_usage(line).filter(|(id, _)| self.last_msg.as_deref() != Some(*id)) {
            self.last_msg = Some(id.into());
            let day = str_field(line, "timestamp")
                .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
                .map(|t| t.with_timezone(&Local).date_naive())
                .unwrap_or_default();
            match self.usage.last_mut() {
                Some((d, acc)) if *d == day => *acc += u,
                _ => self.usage.push((day, u)),
            }
        }
        let interesting = line.contains("\"type\":\"ai-title\"")
            || line.contains("\"type\":\"custom-title\"")
            || (self.first_prompt.is_none() && line.contains("\"type\":\"user\""));
        if !interesting {
            return;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else { return };
        match v["type"].as_str() {
            Some("ai-title") => self.ai_title = v["aiTitle"].as_str().map(one_line),
            Some("custom-title") => self.custom_title = v["customTitle"].as_str().map(one_line),
            Some("user") if self.first_prompt.is_none() => {
                if v["isMeta"].as_bool() == Some(true) || v["isSidechain"].as_bool() == Some(true) {
                    return;
                }
                if let Some(t) = user_text(&v["message"]["content"]) {
                    if !t.starts_with('<') {
                        self.first_prompt = Some(one_line(&t));
                    }
                }
            }
            _ => {}
        }
    }
}

/// Last top-level `"key":"value"` in a JSONL line. Keys nested inside string
/// values are escaped (`\"key\"`) so they can't match.
fn str_field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let pat = format!("\"{key}\":\"");
    let i = line.rfind(&pat)? + pat.len();
    let j = line[i..].find('"')? + i;
    Some(&line[i..j])
}

/// The id and usage of an assistant API message, by string search: these are
/// most lines of a transcript, too many to JSON-parse on every startup.
fn message_usage(line: &str) -> Option<(&str, Usage)> {
    // Assistant messages open with the model; user messages with their role.
    let pat = "\"message\":{\"model\":\"";
    let rest = &line[line.find(pat)? + pat.len()..];
    let id = between(rest, "\"id\":\"", "\"")?;
    let u = &rest[rest.find("\"usage\":{")?..];
    // The first hit of each key is the top-level one; nested copies
    // (`iterations`) come after them, and keys inside strings are escaped.
    let n = |k: &str| -> Option<u64> {
        let pat = format!("\"{k}\":");
        let v = &u[u.find(&pat)? + pat.len()..];
        v[..v.find(|c: char| !c.is_ascii_digit()).unwrap_or(v.len())].parse().ok()
    };
    Some((
        id,
        Usage {
            input: n("input_tokens")?,
            output: n("output_tokens")?,
            cache_write: n("cache_creation_input_tokens").unwrap_or(0),
            cache_read: n("cache_read_input_tokens").unwrap_or(0),
        },
    ))
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn user_text(content: &Value) -> Option<String> {
    match content {
        Value::String(s) => Some(s.clone()),
        Value::Array(a) => {
            let t: Vec<&str> = a.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect();
            (!t.is_empty()).then(|| t.join("\n"))
        }
        _ => None,
    }
}

/// Bump whenever the cache's shape or what `Session::ingest` extracts changes.
const CACHE_VERSION: u64 = 2;

/// Parse state survives restarts here, so startup only reads what was
/// appended since (history runs to hundreds of MB).
pub fn cache_file() -> PathBuf {
    let cache = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".cache"));
    cache.join("cdeck/sessions.json")
}

/// Keeps every transcript we've seen, re-reading only appended bytes.
#[derive(Default)]
pub struct Store {
    pub sessions: HashMap<String, Session>,
    /// Parse state changed since the cache was last written.
    dirty: bool,
}

impl Store {
    pub fn scan(&mut self) {
        self.scan_root(&claude_dir().join("projects"));
    }

    fn scan_root(&mut self, root: &Path) {
        let Ok(dirs) = fs::read_dir(root) else { return };
        let mut seen = std::collections::HashSet::new();
        for d in dirs.flatten() {
            let Ok(files) = fs::read_dir(d.path()) else { continue };
            for f in files.flatten() {
                let p = f.path();
                if p.extension().is_none_or(|e| e != "jsonl") {
                    continue;
                }
                let Some(id) = p.file_stem().and_then(|s| s.to_str()).map(String::from) else { continue };
                let Ok(md) = f.metadata() else { continue };
                seen.insert(id.clone());
                let s = self
                    .sessions
                    .entry(id.clone())
                    .or_insert_with(|| Session { id: id.clone(), file: p.clone(), ..Default::default() });
                if s.file != p {
                    // The same id in two projects: first one wins, so they don't
                    // take turns being reparsed. A moved file starts over.
                    if s.file.exists() {
                        continue;
                    }
                    *s = Session { id, file: p, ..Default::default() };
                }
                s.mtime = md.modified().ok();
                if md.len() != s.offset {
                    let before = s.offset;
                    s.update(md.len());
                    self.dirty |= s.offset != before;
                }
            }
        }
        // Deleted transcripts would otherwise live on in the cache.
        let n = self.sessions.len();
        self.sessions.retain(|id, _| seen.contains(id));
        self.dirty |= self.sessions.len() != n;
    }

    /// Pick up where the last run left off. A missing, corrupt or outdated
    /// cache just means a full scan.
    pub fn load_cache(&mut self) {
        self.load(&cache_file());
    }

    fn load(&mut self, path: &Path) {
        let Ok(txt) = fs::read_to_string(path) else { return };
        let Ok(v) = serde_json::from_str::<Value>(&txt) else { return };
        if v["version"].as_u64() != Some(CACHE_VERSION) {
            return;
        }
        for s in v["sessions"].as_array().into_iter().flatten().filter_map(Session::from_json) {
            self.sessions.insert(s.id.clone(), s);
        }
    }

    /// Write the cache if anything changed; best-effort.
    pub fn save_cache(&mut self) {
        if self.dirty {
            self.save(&cache_file());
        }
    }

    fn save(&mut self, path: &Path) {
        let sessions: Vec<Value> = self.sessions.values().filter_map(Session::to_json).collect();
        let txt = json!({ "version": CACHE_VERSION, "sessions": sessions }).to_string();
        // Write-then-rename so a crash or a second cdeck never leaves half a file.
        let tmp = path.with_extension(format!("tmp{}", std::process::id()));
        let _ = path.parent().map(fs::create_dir_all);
        if fs::write(&tmp, txt).is_ok() && fs::rename(&tmp, path).is_ok() {
            self.dirty = false;
        } else {
            let _ = fs::remove_file(&tmp);
        }
    }

    /// Tokens used today (local time) across every chat.
    pub fn usage_today(&self) -> Usage {
        let today = Local::now().date_naive();
        let mut u = Usage::default();
        for s in self.sessions.values() {
            u += s.usage_since(today);
        }
        u
    }

    pub fn visible(&self) -> impl Iterator<Item = &Session> {
        self.sessions.values().filter(|s| s.has_content() && !s.cwd.as_os_str().is_empty())
    }
}

/// A Claude Code process, as advertised by ~/.claude/sessions/<pid>.json.
#[derive(Clone)]
pub struct Running {
    pub pid: u32,
    pub status: String,
    pub waiting_for: Option<String>,
}

pub fn running() -> HashMap<String, Running> {
    let mut out = HashMap::new();
    let Ok(rd) = fs::read_dir(claude_dir().join("sessions")) else { return out };
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().is_none_or(|x| x != "json") {
            continue;
        }
        let Ok(txt) = fs::read_to_string(&p) else { continue };
        let Ok(v) = serde_json::from_str::<Value>(&txt) else { continue };
        let (Some(pid), Some(id)) = (v["pid"].as_u64(), v["sessionId"].as_str()) else { continue };
        if !Path::new(&format!("/proc/{pid}")).exists() {
            continue;
        }
        out.insert(
            id.into(),
            Running {
                pid: pid as u32,
                status: v["status"].as_str().unwrap_or("").into(),
                waiting_for: v["waitingFor"].as_str().map(String::from),
            },
        );
    }
    out
}

pub enum Entry {
    User(Msg),
    Assistant(Msg),
    Tool(String),
}

/// A message and when it was sent. Derefs to the text, so code that only
/// cares about the words can treat it as a `str`.
pub struct Msg {
    pub text: String,
    pub at: Option<DateTime<Utc>>,
}

impl std::ops::Deref for Msg {
    type Target = str;
    fn deref(&self) -> &str {
        &self.text
    }
}

/// Full transcript for the preview pane.
pub fn transcript(file: &Path) -> Vec<Entry> {
    let mut out = Vec::new();
    let Ok(txt) = fs::read_to_string(file) else { return out };
    for line in txt.lines() {
        line_entries(line, &mut out);
    }
    out
}

/// The displayable entries of one transcript line.
fn line_entries(line: &str, out: &mut Vec<Entry>) {
    if !(line.contains("\"type\":\"user\"") || line.contains("\"type\":\"assistant\"")) {
        return;
    }
    let Ok(v) = serde_json::from_str::<Value>(line) else { return };
    if v["isSidechain"].as_bool() == Some(true) || v["isMeta"].as_bool() == Some(true) {
        return;
    }
    let content = &v["message"]["content"];
    let at = v["timestamp"].as_str().and_then(|t| DateTime::parse_from_rfc3339(t).ok()).map(|t| t.to_utc());
    let msg = |text: String| Msg { text, at };
    match v["type"].as_str() {
        Some("user") => {
            if let Some(t) = user_text(content) {
                if let Some(cmd) = between(&t, "<command-name>", "</command-name>") {
                    out.push(Entry::User(msg(cmd.into())));
                } else if !t.starts_with('<') {
                    out.push(Entry::User(msg(t)));
                }
            }
        }
        Some("assistant") => {
            for b in content.as_array().into_iter().flatten() {
                match b["type"].as_str() {
                    Some("text") => {
                        if let Some(t) = b["text"].as_str().filter(|t| !t.trim().is_empty()) {
                            out.push(Entry::Assistant(msg(t.trim().into())));
                        }
                    }
                    Some("tool_use") => {
                        let name = b["name"].as_str().unwrap_or("?");
                        let i = &b["input"];
                        let arg = ["description", "command", "file_path", "pattern", "url", "query", "prompt"]
                            .iter()
                            .find_map(|k| i[k].as_str())
                            .map(one_line)
                            .unwrap_or_default();
                        out.push(Entry::Tool(format!("{name}({arg})")));
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

/// First user/assistant message in a transcript containing `needle`
/// (already lowercased), as a one-line snippet around the match.
pub fn find_in_transcript(file: &Path, needle: &str, cancelled: impl Fn() -> bool) -> Option<String> {
    let txt = fs::read_to_string(file).ok()?;
    // Cheap raw-bytes prefilter so only candidate lines get JSON-parsed. JSON
    // escapes quotes and backslashes, so needles with those skip it.
    let prefilter = !needle.contains(['"', '\\']);
    let mut entries = Vec::new();
    for (n, line) in txt.lines().enumerate() {
        if n % 256 == 0 && cancelled() {
            return None;
        }
        if prefilter && !contains_ci(line, needle) {
            continue;
        }
        entries.clear();
        line_entries(line, &mut entries);
        for e in &entries {
            let (Entry::User(t) | Entry::Assistant(t)) = e else { continue };
            if let Some(s) = snippet(t, needle) {
                return Some(s);
            }
        }
    }
    None
}

/// Case-insensitive substring test; `needle` must already be lowercase.
fn contains_ci(hay: &str, needle: &str) -> bool {
    if needle.is_ascii() {
        let (h, n) = (hay.as_bytes(), needle.as_bytes());
        n.is_empty() || h.windows(n.len()).any(|w| w.eq_ignore_ascii_case(n))
    } else {
        hay.to_lowercase().contains(needle)
    }
}

/// `…some words around the match…` from `text`, or None if it isn't there.
fn snippet(text: &str, needle: &str) -> Option<String> {
    let flat = one_line(text);
    let lower = flat.to_lowercase();
    // Lowercasing can change byte lengths; map the hit back via char counts.
    let at = lower[..lower.find(needle)?].chars().count();
    let chars: Vec<char> = flat.chars().collect();
    let at = at.min(chars.len());
    // Little lead-in: the sidebar is narrow and the match should stay visible.
    let start = at.saturating_sub(8);
    let end = (at + needle.chars().count() + 80).min(chars.len());
    let mut s: String = chars[start..end].iter().collect();
    if start > 0 {
        s.insert(0, '…');
    }
    if end < chars.len() {
        s.push('…');
    }
    Some(s)
}

fn between<'a>(s: &'a str, a: &str, b: &str) -> Option<&'a str> {
    let i = s.find(a)? + a.len();
    let j = s[i..].find(b)? + i;
    Some(&s[i..j])
}

pub fn new_uuid() -> String {
    fs::read_to_string("/proc/sys/kernel/random/uuid").map(|s| s.trim().to_string()).unwrap_or_else(|_| {
        let n = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let h = format!("{n:032x}");
        format!("{}-{}-4{}-a{}-{}", &h[0..8], &h[8..12], &h[13..16], &h[17..20], &h[20..32])
    })
}

pub fn age(t: Option<SystemTime>) -> String {
    let Some(t) = t else { return String::new() };
    let s = SystemTime::now().duration_since(t).unwrap_or_default().as_secs();
    match s {
        0..60 => "now".into(),
        60..3600 => format!("{}m", s / 60),
        3600..86400 => format!("{}h", s / 3600),
        86400..1_209_600 => format!("{}d", s / 86400),
        _ => format!("{}w", s / 604_800),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippet_centres_on_match_case_insensitively() {
        let text = format!("{} Needle here {}", "a ".repeat(40), "b ".repeat(60));
        let s = snippet(&text, "needle").unwrap();
        assert!(s.starts_with('…') && s.ends_with('…'));
        assert!(s.contains("Needle here"));
        assert_eq!(snippet("nothing", "needle"), None);
    }

    #[test]
    fn finds_user_and_assistant_text_but_not_tool_output() {
        let dir = std::env::temp_dir().join(format!("cdeck-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("t.jsonl");
        fs::write(
            &file,
            concat!(
                r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"secret ToolWord"}]}}"#,
                "\n",
                r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Fixed the Borrow checker error"}]}}"#,
                "\n",
                r#"{"type":"user","message":{"content":"café \"quoted\" thing"}}"#,
                "\n",
            ),
        )
        .unwrap();
        let find = |q: &str| find_in_transcript(&file, q, || false);
        assert_eq!(find("borrow checker").as_deref(), Some("…xed the Borrow checker error"));
        assert_eq!(find("toolword"), None);
        assert!(find("café").is_some());
        assert!(find("\"quoted\"").is_some());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn messages_keep_their_timestamp() {
        let mut out = Vec::new();
        line_entries(r#"{"type":"user","timestamp":"2026-10-04T18:54:49.475Z","message":{"content":"hi"}}"#, &mut out);
        line_entries(r#"{"type":"user","message":{"content":"no time"}}"#, &mut out);
        let [Entry::User(a), Entry::User(b)] = &out[..] else { panic!() };
        assert_eq!(a.at.map(|t| t.timestamp()), Some(1_791_140_089));
        assert_eq!((&**a, b.at), ("hi", None));
    }

    #[test]
    fn usage_counts_each_message_once() {
        let line = |id: &str, block: &str, out: u64| {
            format!(
                r#"{{"type":"assistant","timestamp":"2026-10-04T12:00:00Z","message":{{"model":"m","id":"{id}","content":[{block}],"usage":{{"input_tokens":2,"cache_creation_input_tokens":10,"cache_read_input_tokens":100,"output_tokens":{out},"output_tokens_details":{{"thinking_tokens":1}},"iterations":[{{"input_tokens":9,"output_tokens":9}}]}}}}}}"#
            )
        };
        let mut s = Session::default();
        s.ingest(line("a", r#"{"type":"text","text":"\"usage\":{\"input_tokens\":7"}"#, 5).as_bytes());
        s.ingest(line("a", r#"{"type":"tool_use","id":"toolu_1","input":{}}"#, 5).as_bytes());
        s.ingest(line("b", r#"{"type":"text","text":"hi"}"#, 3).as_bytes());
        s.ingest(br#"{"type":"user","message":{"role":"user","content":"x"},"toolUseResult":{"usage":{"input_tokens":50}}}"#);
        assert_eq!(s.usage(), Usage { input: 4, output: 8, cache_write: 20, cache_read: 200 });
        assert_eq!(s.usage().input_total(), 224);
        let back = Session::from_json(&s.to_json().unwrap()).unwrap();
        assert_eq!((back.usage(), back.last_msg.as_deref()), (s.usage(), Some("b")));
        assert_eq!(message_usage(&line("c", "", 1)).map(|(id, _)| id), Some("c"));
    }

    #[test]
    fn cache_resumes_parsing_and_survives_rewrites() {
        let dir = std::env::temp_dir().join(format!("cdeck-cache-test-{}", std::process::id()));
        let root = dir.join("projects");
        fs::create_dir_all(root.join("p")).unwrap();
        let file = root.join("p/abc.jsonl");
        let cache = dir.join("cache.json");
        let user = |t: &str| format!("{{\"type\":\"user\",\"cwd\":\"/x\",\"message\":{{\"content\":\"{t}\"}}}}\n");
        fs::write(&file, user("first")).unwrap();
        let mut a = Store::default();
        a.scan_root(&root);
        a.save(&cache);
        assert!(!a.dirty);

        // Loaded state is trusted: nothing already parsed is read again.
        let txt = fs::read_to_string(&cache).unwrap().replace("\"first\"", "\"from cache\"");
        fs::write(&cache, &txt).unwrap();
        let mut b = Store::default();
        b.load(&cache);
        b.scan_root(&root);
        assert_eq!(b.sessions["abc"].title(), "from cache");
        assert_eq!(b.sessions["abc"].cwd, Path::new("/x"));
        assert!(!b.dirty);

        // Appended titles are picked up.
        fs::write(&file, user("first") + "{\"type\":\"ai-title\",\"aiTitle\":\"Named\"}\n").unwrap();
        b.scan_root(&root);
        assert_eq!(b.sessions["abc"].title(), "Named");
        assert!(b.dirty);

        // Shorter than what was parsed: rewritten, so start over.
        fs::write(&file, user("new")).unwrap();
        fs::write(&cache, txt.replace(&format!("\"offset\":{}", user("first").len()), "\"offset\":999")).unwrap();
        let mut c = Store::default();
        c.load(&cache);
        assert_eq!(c.sessions["abc"].offset, 999);
        c.scan_root(&root);
        assert_eq!(c.sessions["abc"].title(), "new");

        // Corrupt or outdated caches are ignored.
        for bad in ["{not json".to_string(), txt.replace("\"version\":2", "\"version\":1")] {
            fs::write(&cache, bad).unwrap();
            let mut d = Store::default();
            d.load(&cache);
            assert!(d.sessions.is_empty());
        }

        // Deleted transcripts drop out.
        fs::remove_file(&file).unwrap();
        c.scan_root(&root);
        assert!(c.sessions.is_empty() && c.dirty);
        fs::remove_dir_all(&dir).unwrap();
    }

    /// `cargo test --release -- --ignored --nocapture` to time startup's scan of real history.
    #[test]
    #[ignore]
    fn time_startup_scan() {
        let t = std::time::Instant::now();
        let mut store = Store::default();
        store.scan();
        println!("scanned {} sessions in {:?}", store.sessions.len(), t.elapsed());
        let cache = std::env::temp_dir().join(format!("cdeck-time-{}.json", std::process::id()));
        let t = std::time::Instant::now();
        store.save(&cache);
        println!("saved the cache ({} KB) in {:?}", fs::metadata(&cache).unwrap().len() / 1024, t.elapsed());
        let t = std::time::Instant::now();
        let mut store = Store::default();
        store.load(&cache);
        store.scan();
        println!("loaded the cache and rescanned in {:?}", t.elapsed());
        fs::remove_file(&cache).unwrap();
    }

    /// `cargo test -- --ignored --nocapture` to time a search over real history.
    #[test]
    #[ignore]
    fn time_real_history() {
        let mut store = Store::default();
        store.scan();
        let t = std::time::Instant::now();
        let hits = store.visible().filter(|s| find_in_transcript(&s.file, "error", || false).is_some()).count();
        println!("{hits}/{} chats in {:?}", store.visible().count(), t.elapsed());
    }
}
