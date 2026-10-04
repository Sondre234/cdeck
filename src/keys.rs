//! Encode crossterm key events as the legacy xterm byte sequences a program
//! inside the PTY expects.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers as M};

pub fn encode(k: KeyEvent, app_cursor: bool) -> Vec<u8> {
    let ctrl = k.modifiers.contains(M::CONTROL);
    let alt = k.modifiers.contains(M::ALT);
    let shift = k.modifiers.contains(M::SHIFT);
    // xterm modifier parameter: 1 + shift + 2*alt + 4*ctrl
    let m = 1 + shift as u8 + 2 * alt as u8 + 4 * ctrl as u8;
    let esc = |s: &str| -> Vec<u8> {
        let mut v = if alt { vec![0x1b] } else { vec![] };
        v.extend_from_slice(s.as_bytes());
        v
    };
    let cursor = |c: char| -> Vec<u8> {
        if m > 1 {
            format!("\x1b[1;{m}{c}").into_bytes()
        } else if app_cursor {
            format!("\x1bO{c}").into_bytes()
        } else {
            format!("\x1b[{c}").into_bytes()
        }
    };
    let tilde = |n: u8| -> Vec<u8> {
        if m > 1 { format!("\x1b[{n};{m}~").into_bytes() } else { format!("\x1b[{n}~").into_bytes() }
    };
    match k.code {
        KeyCode::Char(c) if ctrl => {
            let b = match c.to_ascii_lowercase() {
                c @ 'a'..='z' => c as u8 - b'a' + 1,
                '@' | ' ' | '2' => 0,
                '[' | '3' => 0x1b,
                '\\' | '4' => 0x1c,
                ']' | '5' => 0x1d,
                '^' | '6' => 0x1e,
                '_' | '-' | '7' => 0x1f,
                '?' | '8' => 0x7f,
                _ => return esc(&c.to_string()),
            };
            if alt { vec![0x1b, b] } else { vec![b] }
        }
        KeyCode::Char(c) => esc(&c.to_string()),
        // Shift/Alt+Enter: Claude Code treats ESC CR as "insert newline".
        KeyCode::Enter if shift || alt => b"\x1b\r".to_vec(),
        KeyCode::Enter => b"\r".to_vec(),
        KeyCode::Tab => b"\t".to_vec(),
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Backspace if ctrl => vec![0x08],
        KeyCode::Backspace => esc("\x7f"),
        KeyCode::Esc => b"\x1b".to_vec(),
        KeyCode::Up => cursor('A'),
        KeyCode::Down => cursor('B'),
        KeyCode::Right => cursor('C'),
        KeyCode::Left => cursor('D'),
        KeyCode::Home => cursor('H'),
        KeyCode::End => cursor('F'),
        KeyCode::Insert => tilde(2),
        KeyCode::Delete => tilde(3),
        KeyCode::PageUp => tilde(5),
        KeyCode::PageDown => tilde(6),
        KeyCode::F(n @ 1..=4) => format!("\x1bO{}", (b'P' + n - 1) as char).into_bytes(),
        KeyCode::F(n) => {
            let code = match n {
                5 => 15,
                6 => 17,
                7 => 18,
                8 => 19,
                9 => 20,
                10 => 21,
                11 => 23,
                12 => 24,
                _ => return vec![],
            };
            tilde(code)
        }
        _ => vec![],
    }
}
