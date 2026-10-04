//! Copying chats to the system clipboard: `y` (Claude's last reply) and `Y`
//! (the whole chat as Markdown).

use crate::data::Entry;
use std::io::Write;
use std::process::{Command, Stdio};

/// Claude's text from the last turn that had any (tool calls skipped), so a
/// turn still running or ending on a tool call falls back to the one before.
pub fn last_reply(entries: &[Entry]) -> Option<String> {
    entries
        .split(|e| matches!(e, Entry::User(_)))
        .rev()
        .map(|turn| turn.iter().filter_map(|e| if let Entry::Assistant(t) = e { Some(&t.text[..]) } else { None }).collect::<Vec<_>>())
        .find(|texts| !texts.is_empty())
        .map(|texts| texts.join("\n\n"))
}

/// The chat as Markdown: `## You` / `## Claude` sections, tool calls quoted.
pub fn markdown(title: &str, entries: &[Entry]) -> String {
    let mut out = format!("# {title}\n");
    let mut claude = false;
    for e in entries {
        let (head, body) = match e {
            Entry::User(t) => (Some("You"), t.text.clone()),
            Entry::Assistant(t) => ((!claude).then_some("Claude"), t.text.clone()),
            Entry::Tool(t) => ((!claude).then_some("Claude"), format!("> ⚙ {}", t.call)),
        };
        claude = !matches!(e, Entry::User(_));
        if let Some(h) = head {
            out += &format!("\n## {h}\n");
        }
        out += &format!("\n{body}\n");
    }
    out
}

/// Put `text` on the clipboard: wl-copy under Wayland, xclip under X, else
/// OSC 52 so the terminal does it (works over ssh too). Names what was used.
pub fn copy(text: &str) -> Result<&'static str, String> {
    let env = |k: &str| std::env::var_os(k).is_some_and(|v| !v.is_empty());
    let tools: [(&str, &[&str], bool); 2] =
        [("wl-copy", &[], env("WAYLAND_DISPLAY")), ("xclip", &["-selection", "clipboard"], env("DISPLAY"))];
    for (prog, args, usable) in tools {
        if !usable {
            continue;
        }
        // Not found: try the next way. Both fork to keep serving the selection.
        let Ok(mut child) = Command::new(prog).args(args).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()
        else {
            continue;
        };
        let wrote = child.stdin.take().map(|mut i| i.write_all(text.as_bytes()));
        std::thread::spawn(move || child.wait());
        return match wrote {
            Some(Ok(())) => Ok(prog),
            _ => Err(format!("{prog} didn't take the text")),
        };
    }
    // Written between frames, so it can't land inside ratatui's output.
    let mut out = std::io::stdout();
    write!(out, "\x1b]52;c;{}\x07", base64(text.as_bytes())).and_then(|_| out.flush()).map_err(|e| e.to_string())?;
    Ok("the terminal (OSC 52)")
}

fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = c.iter().enumerate().fold(0u32, |n, (i, &b)| n | (b as u32) << (16 - 8 * i));
        for i in 0..4 {
            s.push(if i <= c.len() { ABC[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::{Msg, Tool};

    fn msg(t: &str) -> Msg {
        Msg { text: t.into(), at: None }
    }

    fn chat() -> Vec<Entry> {
        vec![
            Entry::User(msg("fix it")),
            Entry::Assistant(msg("Looking.")),
            Entry::Tool(Tool::new("Bash(ls)")),
            Entry::Assistant(msg("Fixed.")),
            Entry::User(msg("thanks")),
            Entry::Tool(Tool::new("Read(/x)")),
        ]
    }

    #[test]
    fn last_reply_skips_turns_without_text() {
        assert_eq!(last_reply(&chat()).as_deref(), Some("Looking.\n\nFixed."));
        assert_eq!(last_reply(&chat()[..1]), None);
    }

    #[test]
    fn markdown_has_one_heading_per_turn() {
        assert_eq!(
            markdown("T", &chat()),
            "# T\n\n## You\n\nfix it\n\n## Claude\n\nLooking.\n\n> ⚙ Bash(ls)\n\nFixed.\n\n## You\n\nthanks\n\n## Claude\n\n> ⚙ Read(/x)\n"
        );
    }

    #[test]
    fn base64_pads() {
        assert_eq!(["", "f", "fo", "foo", "foob", "é"].map(|s| base64(s.as_bytes())), ["", "Zg==", "Zm8=", "Zm9v", "Zm9vYg==", "w6k="]);
    }
}
