//! Opening a chat in its own terminal window, outside cdeck.

use std::ffi::OsString;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};

/// Tried in order when $TERMINAL isn't set.
const TERMINALS: [&str; 6] = ["kitty", "foot", "alacritty", "wezterm", "ghostty", "xterm"];

/// $TERMINAL (which may carry its own flags), else the first known one on $PATH.
pub fn terminal() -> Option<Vec<String>> {
    if let Some(t) = std::env::var("TERMINAL").ok().filter(|t| !t.trim().is_empty()) {
        return Some(t.split_whitespace().map(String::from).collect());
    }
    TERMINALS.iter().find(|t| on_path(t)).map(|t| vec![t.to_string()])
}

fn on_path(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(name).is_file()))
}

/// Full argv for running `cmd` in `dir` in a new window of `term`. Every
/// terminal spells "start here, run this" differently; unknown ones get `-e`
/// and a shell `cd`, with the paths passed as arguments so nothing needs quoting.
pub fn command(term: &[String], dir: &Path, cmd: &[String]) -> Vec<OsString> {
    let mut v: Vec<OsString> = term.iter().map(OsString::from).collect();
    let name = term.first().and_then(|t| Path::new(t).file_name()).and_then(|n| n.to_str()).unwrap_or("");
    let before_dir: &[&str] = match name {
        "kitty" => &["--directory"],
        "foot" => &["-D"],
        "alacritty" => &["--working-directory"],
        "wezterm" => &["start", "--cwd"],
        "ghostty" => &[],
        _ => &["-e", "sh", "-c", r#"cd "$1" && shift && exec "$@""#, "sh"],
    };
    v.extend(before_dir.iter().map(OsString::from));
    if name == "ghostty" {
        let mut a = OsString::from("--working-directory=");
        a.push(dir);
        v.push(a);
    } else {
        v.push(dir.into());
    }
    match name {
        "alacritty" | "ghostty" => v.push("-e".into()),
        "wezterm" => v.push("--".into()),
        _ => {}
    }
    v.extend(cmd.iter().map(OsString::from));
    v
}

/// Start it detached: its own process group, no stdio, so it neither scribbles
/// over cdeck nor dies with it. A thread reaps it to avoid a zombie.
pub fn launch(argv: &[OsString], dir: &Path) -> std::io::Result<()> {
    let (prog, args) = argv.split_first().ok_or_else(|| std::io::Error::other("empty command"))?;
    let mut child = Command::new(prog)
        .args(args)
        .current_dir(dir)
        .env_remove("CLAUDECODE")
        .env_remove("CLAUDE_CODE_ENTRYPOINT")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()?;
    std::thread::spawn(move || child.wait());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(term: &str) -> Vec<String> {
        let term: Vec<String> = term.split_whitespace().map(String::from).collect();
        let cmd = ["claude", "--resume", "abc"].map(String::from);
        command(&term, Path::new("/my dir"), &cmd).into_iter().map(|s| s.into_string().unwrap()).collect()
    }

    #[test]
    fn each_terminal_gets_its_own_cwd_flag() {
        assert_eq!(argv("kitty"), ["kitty", "--directory", "/my dir", "claude", "--resume", "abc"]);
        assert_eq!(argv("/usr/bin/foot"), ["/usr/bin/foot", "-D", "/my dir", "claude", "--resume", "abc"]);
        assert_eq!(argv("alacritty"), ["alacritty", "--working-directory", "/my dir", "-e", "claude", "--resume", "abc"]);
        assert_eq!(argv("wezterm"), ["wezterm", "start", "--cwd", "/my dir", "--", "claude", "--resume", "abc"]);
        assert_eq!(argv("ghostty"), ["ghostty", "--working-directory=/my dir", "-e", "claude", "--resume", "abc"]);
    }

    #[test]
    fn unknown_terminals_cd_through_a_shell() {
        let v = argv("xterm -fa Mono");
        assert_eq!(v[..4], ["xterm", "-fa", "Mono", "-e"]);
        assert_eq!(v[4..], ["sh", "-c", r#"cd "$1" && shift && exec "$@""#, "sh", "/my dir", "claude", "--resume", "abc"]);
    }
}
