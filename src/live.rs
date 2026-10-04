//! Claude Code instances running inside cdeck, each on its own PTY and
//! rendered through a vt100 emulator.

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

type Writer = Arc<Mutex<Box<dyn Write + Send>>>;

/// Terminal side-channel: query replies, title, bell, clipboard.
#[derive(Default)]
pub struct Cb {
    pub title: String,
    pub bell: bool,
    reply: Vec<u8>,
    clipboard: Vec<Vec<u8>>,
}

impl vt100::Callbacks for Cb {
    fn audible_bell(&mut self, _: &mut vt100::Screen) {
        self.bell = true;
    }
    fn set_window_title(&mut self, _: &mut vt100::Screen, t: &[u8]) {
        self.title = String::from_utf8_lossy(t).trim().to_string();
    }
    fn copy_to_clipboard(&mut self, _: &mut vt100::Screen, ty: &[u8], data: &[u8]) {
        // Re-emit OSC 52 to the outer terminal so /copy still works.
        let mut seq = b"\x1b]52;".to_vec();
        seq.extend_from_slice(ty);
        seq.push(b';');
        seq.extend_from_slice(data);
        seq.extend_from_slice(b"\x1b\\");
        self.clipboard.push(seq);
    }
    fn unhandled_csi(&mut self, s: &mut vt100::Screen, i1: Option<u8>, _: Option<u8>, params: &[&[u16]], c: char) {
        let p0 = params.first().and_then(|p| p.first()).copied().unwrap_or(0);
        match (i1, c, p0) {
            // Cursor position report.
            (None, 'n', 6) => {
                let (r, col) = s.cursor_position();
                self.reply.extend(format!("\x1b[{};{}R", r + 1, col + 1).bytes());
            }
            (None, 'n', 5) => self.reply.extend(b"\x1b[0n"),
            // Primary device attributes: a plain VT220.
            (None, 'c', 0) => self.reply.extend(b"\x1b[?62;22c"),
            _ => {}
        }
    }
    fn unhandled_osc(&mut self, _: &mut vt100::Screen, params: &[&[u8]]) {
        // Background/foreground colour queries: claim a dark terminal.
        if params.len() == 2 && params[1] == b"?" {
            match params[0] {
                b"10" => self.reply.extend(b"\x1b]10;rgb:dddd/dddd/dddd\x1b\\"),
                b"11" => self.reply.extend(b"\x1b]11;rgb:1111/1111/1111\x1b\\"),
                _ => {}
            }
        }
    }
}

pub struct Live {
    pub id: String,
    pub cwd: PathBuf,
    pub pid: Option<u32>,
    pub parser: Arc<Mutex<vt100::Parser<Cb>>>,
    writer: Writer,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    last_output: Arc<Mutex<Instant>>,
    pub exited: Arc<AtomicBool>,
    pub size: (u16, u16),
    pub started: Instant,
    /// Finished work while you weren't looking at it.
    pub unseen: bool,
}

/// Bumped by every reader thread; the UI redraws when it changes.
pub static GENERATION: AtomicU64 = AtomicU64::new(0);

/// OSC 52 sequences waiting for the main thread. Writing them from the reader
/// thread could land in the middle of a frame ratatui is drawing.
static CLIPBOARD: Mutex<Vec<Vec<u8>>> = Mutex::new(Vec::new());

pub fn take_clipboard() -> Vec<Vec<u8>> {
    std::mem::take(&mut CLIPBOARD.lock().unwrap())
}

impl Live {
    pub fn spawn(id: &str, cwd: &Path, args: &[String], size: (u16, u16)) -> anyhow_lite::Result<Live> {
        let pty = native_pty_system().openpty(PtySize { rows: size.0, cols: size.1, pixel_width: 0, pixel_height: 0 })?;
        let prog = std::env::var("CDECK_CLAUDE").unwrap_or_else(|_| "claude".into());
        let mut cmd = CommandBuilder::new(prog);
        cmd.args(args);
        cmd.cwd(cwd);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        // Don't let a parent Claude Code leak "you are nested" into children.
        cmd.env_remove("CLAUDECODE");
        cmd.env_remove("CLAUDE_CODE_ENTRYPOINT");
        let child = pty.slave.spawn_command(cmd)?;
        drop(pty.slave);
        let pid = child.process_id();
        let mut reader = pty.master.try_clone_reader()?;
        let writer: Writer = Arc::new(Mutex::new(pty.master.take_writer()?));
        let parser = Arc::new(Mutex::new(vt100::Parser::new_with_callbacks(size.0, size.1, 5000, Cb::default())));
        let last_output = Arc::new(Mutex::new(Instant::now()));
        let exited = Arc::new(AtomicBool::new(false));
        {
            let (parser, writer, last_output, exited) =
                (parser.clone(), writer.clone(), last_output.clone(), exited.clone());
            std::thread::spawn(move || {
                let mut buf = [0u8; 16384];
                loop {
                    let n = match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    let (reply, clip) = {
                        let mut p = parser.lock().unwrap();
                        p.process(&buf[..n]);
                        let cb = p.callbacks_mut();
                        (std::mem::take(&mut cb.reply), std::mem::take(&mut cb.clipboard))
                    };
                    if !reply.is_empty() {
                        let mut w = writer.lock().unwrap();
                        let _ = w.write_all(&reply);
                        let _ = w.flush();
                    }
                    if !clip.is_empty() {
                        CLIPBOARD.lock().unwrap().extend(clip);
                    }
                    *last_output.lock().unwrap() = Instant::now();
                    GENERATION.fetch_add(1, Ordering::Relaxed);
                }
                exited.store(true, Ordering::Relaxed);
                GENERATION.fetch_add(1, Ordering::Relaxed);
            });
        }
        Ok(Live {
            id: id.into(),
            cwd: cwd.into(),
            pid,
            parser,
            writer,
            master: pty.master,
            child,
            last_output,
            exited,
            size,
            started: Instant::now(),
            unseen: false,
        })
    }

    pub fn write(&self, bytes: &[u8]) {
        let mut w = self.writer.lock().unwrap();
        let _ = w.write_all(bytes);
        let _ = w.flush();
    }

    pub fn paste(&self, text: &str) {
        let bracketed = self.parser.lock().unwrap().screen().bracketed_paste();
        if bracketed {
            self.write(format!("\x1b[200~{text}\x1b[201~").as_bytes());
        } else {
            self.write(text.as_bytes());
        }
    }

    pub fn resize(&mut self, size: (u16, u16)) {
        if size == self.size || size.0 == 0 || size.1 == 0 {
            return;
        }
        self.size = size;
        let _ = self.master.resize(PtySize { rows: size.0, cols: size.1, pixel_width: 0, pixel_height: 0 });
        self.parser.lock().unwrap().screen_mut().set_size(size.0, size.1);
    }

    pub fn busy(&self) -> bool {
        self.last_output.lock().unwrap().elapsed() < Duration::from_millis(1200)
    }

    /// Positive `delta` scrolls up. Fullscreen Claude Code lives on the
    /// alternate screen with mouse tracking on, so there's no scrollback to
    /// show; it scrolls itself when sent wheel events.
    pub fn scroll(&self, delta: isize) {
        let mut p = self.parser.lock().unwrap();
        let s = p.screen_mut();
        if s.mouse_protocol_mode() == vt100::MouseProtocolMode::None {
            let cur = s.scrollback() as isize;
            s.set_scrollback((cur + delta).max(0) as usize);
            return;
        }
        // One wheel notch per 3 lines, like a real wheel; Home/End get capped.
        let ticks = (delta.unsigned_abs() / 3).clamp(1, 100);
        let button = if delta > 0 { 64 } else { 65 };
        let (row, col) = (self.size.0 / 2 + 1, self.size.1 / 2 + 1);
        let one = match s.mouse_protocol_encoding() {
            vt100::MouseProtocolEncoding::Sgr => format!("\x1b[<{button};{col};{row}M").into_bytes(),
            _ => vec![0x1b, b'[', b'M', 32 + button as u8, 32 + col.min(223) as u8, 32 + row.min(223) as u8],
        };
        drop(p);
        self.write(&one.repeat(ticks));
    }

    pub fn reset_scroll(&self) {
        self.parser.lock().unwrap().screen_mut().set_scrollback(0);
    }

    pub fn kill(&mut self) {
        let _ = self.child.kill();
    }

    pub fn reap(&mut self) -> bool {
        self.exited.load(Ordering::Relaxed) || matches!(self.child.try_wait(), Ok(Some(_)))
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        self.kill();
    }
}

/// portable-pty returns anyhow errors; keep our own surface tiny.
pub mod anyhow_lite {
    pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
}
