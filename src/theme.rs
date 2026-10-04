//! Colours come from the terminal and from the active Claude Code theme.
//!
//! Backgrounds stay `Reset` so the terminal's own background (and opacity)
//! shows through. Accents are read from `~/.claude/themes/<slug>.json` when
//! settings.json says `"theme": "custom:<slug>"`; anything the theme doesn't
//! set falls back to the terminal's ANSI palette. The files are re-read when
//! they change, so switching Claude themes recolours cdeck live.

use crate::data::claude_dir;
use ratatui::style::Color;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::RwLock;
use std::time::SystemTime;

#[derive(Clone, Copy)]
pub struct Theme {
    pub accent: Color,
    pub shimmer: Color,
    pub user: Color,
    pub inverse: Color,
    pub muted: Color,
    pub faint: Color,
    pub border: Color,
    pub prompt_border: Color,
    pub sel: Color,
    pub sel_dim: Color,
    pub bubble: Color,
    pub code: Color,
    pub success: Color,
    pub warning: Color,
    pub error: Color,
    pub permission: Color,
    pub external: Color,
    pub lilac: Color,
    pub dirs: [Color; 8],
    pub name: Option<&'static str>,
}

impl Theme {
    fn ansi() -> Theme {
        Theme {
            accent: Color::LightRed,
            shimmer: Color::LightYellow,
            user: Color::Reset,
            inverse: Color::Black,
            muted: Color::Gray,
            faint: Color::DarkGray,
            border: Color::DarkGray,
            prompt_border: Color::DarkGray,
            sel: Color::DarkGray,
            sel_dim: Color::Reset,
            bubble: Color::Reset,
            code: Color::Cyan,
            success: Color::Green,
            warning: Color::Yellow,
            error: Color::Red,
            permission: Color::Blue,
            external: Color::Blue,
            lilac: Color::Magenta,
            dirs: [
                Color::Cyan,
                Color::Magenta,
                Color::Yellow,
                Color::Green,
                Color::Blue,
                Color::LightCyan,
                Color::LightMagenta,
                Color::LightGreen,
            ],
            name: None,
        }
    }
}

/// Claude Code's colour syntax: `#rgb`, `#rrggbb`, `rgb(r,g,b)`, `ansi256(n)`, `ansi:name`.
fn parse(s: &str) -> Option<Color> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        let hex: String = if hex.len() == 3 { hex.chars().flat_map(|c| [c, c]).collect() } else { hex.into() };
        let n = u32::from_str_radix(&hex, 16).ok()?;
        return (hex.len() == 6).then(|| Color::Rgb((n >> 16) as u8, (n >> 8) as u8, n as u8));
    }
    if let Some(body) = s.strip_prefix("rgb(").and_then(|b| b.strip_suffix(')')) {
        let v: Vec<u8> = body.split(',').filter_map(|x| x.trim().parse().ok()).collect();
        return (v.len() == 3).then(|| Color::Rgb(v[0], v[1], v[2]));
    }
    if let Some(n) = s.strip_prefix("ansi256(").and_then(|b| b.strip_suffix(')')) {
        return n.trim().parse().ok().map(Color::Indexed);
    }
    Some(match s.strip_prefix("ansi:")? {
        "black" => Color::Black,
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "blue" => Color::Blue,
        "magenta" => Color::Magenta,
        "cyan" => Color::Cyan,
        "white" => Color::Gray,
        "blackBright" => Color::DarkGray,
        "redBright" => Color::LightRed,
        "greenBright" => Color::LightGreen,
        "yellowBright" => Color::LightYellow,
        "blueBright" => Color::LightBlue,
        "magentaBright" => Color::LightMagenta,
        "cyanBright" => Color::LightCyan,
        "whiteBright" => Color::White,
        _ => return None,
    })
}

fn theme_file() -> Option<PathBuf> {
    let settings = std::fs::read_to_string(claude_dir().join("settings.json")).ok()?;
    let v: Value = serde_json::from_str(&settings).ok()?;
    let slug = v["theme"].as_str()?.strip_prefix("custom:")?;
    Some(claude_dir().join("themes").join(format!("{slug}.json")))
}

fn load() -> Theme {
    let mut t = Theme::ansi();
    let Some(v) = theme_file()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
    else {
        return t;
    };
    let o = &v["overrides"];
    let get = |k: &str| o[k].as_str().and_then(parse);
    let set = |slot: &mut Color, keys: &[&str]| {
        if let Some(c) = keys.iter().find_map(|k| get(k)) {
            *slot = c;
        }
    };
    set(&mut t.accent, &["claude"]);
    set(&mut t.shimmer, &["claudeShimmer"]);
    set(&mut t.user, &["text"]);
    set(&mut t.inverse, &["inverseText"]);
    set(&mut t.muted, &["inactiveShimmer", "inactive"]);
    set(&mut t.faint, &["inactive"]);
    set(&mut t.border, &["subtle"]);
    set(&mut t.prompt_border, &["promptBorder"]);
    set(&mut t.sel, &["selectionBg"]);
    set(&mut t.sel_dim, &["userMessageBackgroundHover", "composerSidebarBackground"]);
    set(&mut t.bubble, &["userMessageBackground"]);
    set(&mut t.code, &["suggestion"]);
    set(&mut t.success, &["success"]);
    set(&mut t.warning, &["warning"]);
    set(&mut t.error, &["red_FOR_SUBAGENTS_ONLY", "bashBorder"]);
    set(&mut t.permission, &["permission"]);
    set(&mut t.external, &["ide", "claudeBlue_FOR_SYSTEM_SPINNER"]);
    set(&mut t.lilac, &["remember", "autoAccept"]);
    let agents = ["cyan", "purple", "yellow", "green", "blue", "pink", "orange", "red"];
    for (slot, name) in t.dirs.iter_mut().zip(agents) {
        set(slot, &[&format!("{name}_FOR_SUBAGENTS_ONLY")]);
    }
    // Leaked once per reload; themes change rarely.
    t.name = v["name"].as_str().map(|s| &*Box::leak(s.to_string().into_boxed_str()));
    t
}

static THEME: RwLock<Option<Theme>> = RwLock::new(None);
static STAMP: RwLock<Option<(Option<SystemTime>, Option<SystemTime>)>> = RwLock::new(None);

pub fn th() -> Theme {
    if let Some(t) = *THEME.read().unwrap() {
        return t;
    }
    let t = load();
    *THEME.write().unwrap() = Some(t);
    t
}

/// Re-read the theme if settings.json or the theme file changed. Returns true on change.
pub fn reload_if_changed() -> bool {
    let mtime = |p: Option<PathBuf>| p.and_then(|p| std::fs::metadata(p).ok()).and_then(|m| m.modified().ok());
    let stamp = (mtime(Some(claude_dir().join("settings.json"))), mtime(theme_file()));
    let prev = STAMP.write().unwrap().replace(stamp);
    if prev.is_some_and(|p| p != stamp) {
        *THEME.write().unwrap() = Some(load());
        return true;
    }
    false
}

fn mix(a: Color, b: Color, t: f32) -> Color {
    match (a, b) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => {
            let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
            Color::Rgb(l(r1, r2), l(g1, g2), l(b1, b2))
        }
        _ if t >= 0.5 => b,
        _ => a,
    }
}

/// Claude Code-style shimmer: a bright band sweeping across the text.
pub fn shimmer(text: &str, base: Color, hi: Color, tick: usize) -> Vec<(String, Color)> {
    let n = text.chars().count();
    let pos = (tick % (n + 8)) as isize - 4;
    text.chars()
        .enumerate()
        .map(|(i, c)| {
            let d = (i as isize - pos).unsigned_abs() as f32;
            (c.to_string(), mix(base, hi, (1.0 - d / 3.0).max(0.0)))
        })
        .collect()
}

/// Static left-to-right gradient.
pub fn gradient(text: &str, a: Color, b: Color) -> Vec<(String, Color)> {
    let n = text.chars().count().max(2) - 1;
    text.chars().enumerate().map(|(i, c)| (c.to_string(), mix(a, b, i as f32 / n as f32))).collect()
}
