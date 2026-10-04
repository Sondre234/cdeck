//! Reading Claude Code's on-disk state: session transcripts under
//! ~/.claude/projects and the per-process status files under ~/.claude/sessions.

use serde_json::Value;
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
    // Incremental parse state: transcripts are append-only.
    offset: u64,
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

/// Keeps every transcript we've seen, re-reading only appended bytes.
#[derive(Default)]
pub struct Store {
    pub sessions: HashMap<String, Session>,
}

impl Store {
    pub fn scan(&mut self) {
        let root = claude_dir().join("projects");
        let Ok(dirs) = fs::read_dir(&root) else { return };
        for d in dirs.flatten() {
            let Ok(files) = fs::read_dir(d.path()) else { continue };
            for f in files.flatten() {
                let p = f.path();
                if p.extension().is_none_or(|e| e != "jsonl") {
                    continue;
                }
                let Some(id) = p.file_stem().and_then(|s| s.to_str()).map(String::from) else { continue };
                let Ok(md) = f.metadata() else { continue };
                let s = self
                    .sessions
                    .entry(id.clone())
                    .or_insert_with(|| Session { id, file: p.clone(), ..Default::default() });
                s.file = p;
                s.mtime = md.modified().ok();
                if md.len() != s.offset {
                    s.update(md.len());
                }
            }
        }
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
    User(String),
    Assistant(String),
    Tool(String),
}

/// Full transcript for the preview pane.
pub fn transcript(file: &Path) -> Vec<Entry> {
    let mut out = Vec::new();
    let Ok(txt) = fs::read_to_string(file) else { return out };
    for line in txt.lines() {
        if !(line.contains("\"type\":\"user\"") || line.contains("\"type\":\"assistant\"")) {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if v["isSidechain"].as_bool() == Some(true) || v["isMeta"].as_bool() == Some(true) {
            continue;
        }
        let content = &v["message"]["content"];
        match v["type"].as_str() {
            Some("user") => {
                if let Some(t) = user_text(content) {
                    if let Some(cmd) = between(&t, "<command-name>", "</command-name>") {
                        out.push(Entry::User(cmd.into()));
                    } else if !t.starts_with('<') {
                        out.push(Entry::User(t));
                    }
                }
            }
            Some("assistant") => {
                for b in content.as_array().into_iter().flatten() {
                    match b["type"].as_str() {
                        Some("text") => {
                            if let Some(t) = b["text"].as_str().filter(|t| !t.trim().is_empty()) {
                                out.push(Entry::Assistant(t.trim().into()));
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
    out
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
