//! User settings from `$XDG_CONFIG_HOME/cdeck/config.toml`.
//!
//! Read once at startup. A missing file means defaults; unknown keys are
//! ignored so older builds keep working with newer configs; a file that
//! doesn't parse falls back to defaults and the error is shown once.

use serde::Deserialize;
use std::path::PathBuf;
use std::sync::OnceLock;

#[derive(Deserialize, Debug, PartialEq)]
#[serde(default)]
pub struct Config {
    /// Below this many columns the list and the chat take turns.
    pub narrow_width: u16,
    /// Chats a directory shows before folding the rest behind `z`.
    pub group_limit: usize,
    /// Terminal for `E`, overriding $TERMINAL (may carry its own flags).
    pub terminal: Option<String>,
    /// Fixed chat-list width; unset sizes it to the window.
    pub sidebar_width: Option<u16>,
}

impl Default for Config {
    fn default() -> Self {
        Config { narrow_width: 100, group_limit: 5, terminal: None, sidebar_width: None }
    }
}

pub fn path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| crate::data::home().join(".config"));
    base.join("cdeck/config.toml")
}

fn parse(s: &str) -> Result<Config, String> {
    toml::from_str(s).map_err(|e| e.message().to_string())
}

static CONFIG: OnceLock<(Config, Option<String>)> = OnceLock::new();

fn loaded() -> &'static (Config, Option<String>) {
    CONFIG.get_or_init(|| match std::fs::read_to_string(path()) {
        Err(_) => (Config::default(), None),
        Ok(s) => match parse(&s) {
            Ok(c) => (c, None),
            Err(e) => (Config::default(), Some(format!("config.toml: {e} — using defaults"))),
        },
    })
}

pub fn cfg() -> &'static Config {
    &loaded().0
}

/// Why the config file was ignored, if it was.
pub fn error() -> Option<&'static str> {
    loaded().1.as_deref()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_is_defaults() {
        assert_eq!(parse("").unwrap(), Config::default());
    }

    #[test]
    fn reads_every_setting_and_ignores_unknown_keys() {
        let c = parse("narrow_width = 120\ngroup_limit = 3\nterminal = \"foot -o x\"\nsidebar_width = 40\nfuture = true\n").unwrap();
        assert_eq!(c, Config { narrow_width: 120, group_limit: 3, terminal: Some("foot -o x".into()), sidebar_width: Some(40) });
    }

    #[test]
    fn partial_file_keeps_other_defaults() {
        let c = parse("group_limit = 10").unwrap();
        assert_eq!(c, Config { group_limit: 10, ..Config::default() });
    }

    #[test]
    fn bad_values_are_errors() {
        assert!(parse("narrow_width = \"wide\"").is_err());
        assert!(parse("narrow_width = ").is_err());
    }
}
