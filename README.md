# cdeck

A terminal deck for [Claude Code](https://claude.com/claude-code): every chat
you've had, grouped by directory, with live Claude sessions running in a pane
next to the list. Helix-style keys, claude.ai-style new-chat screen.

- Browse and full-text search all past chats from `~/.claude/projects`
- Resume, fork, rename, pin and archive chats; run several live sessions side by side
- See at a glance which sessions are working, waiting on you, or done
- Desktop notifications when a background chat finishes or needs you
- Adapts to narrow (tiled) windows by showing either the list or the chat

## Install

```sh
cargo install --path .
```

Needs `claude` on your `$PATH`. Optional: `notify-send` (notifications),
`zoxide` (more directories in the picker), a terminal emulator for `E`.

## Layout

The chat list (sidebar) is on the left, the pane on the right. The pane shows
the selected chat: a live Claude session if it's running in cdeck, otherwise a
read-only preview of its transcript.

Below 100 columns (`narrow_width` in the [config](#config); e.g. a half-width window in a tiling WM) only one of them is
shown at a time: whichever has focus. `Ctrl-\` from a chat brings the list
back; opening a chat shows it.

Directories with many chats show their 5 newest (`group_limit`); `z` expands them.

## Modes

The footer badge shows the current mode.

| Badge  | Mode    | What keys do                                   |
|--------|---------|------------------------------------------------|
| `NOR`  | normal  | navigate, open, manage chats                   |
| `INS`  | typing  | everything goes to Claude except `Ctrl-\`      |
| `NEW`  | compose | writing the first message of a new chat        |
| `FIND` | search  | typing into the `/` search box                 |
| `CMD`  | command | typing a `:` command                           |
| `SPC` `GOTO` `WIN` | prefix | waiting for the second key of `space`, `g`, `Ctrl-w` |

## Keys

### Chat list

| Key | Action |
|-----|--------|
| `j` `k` / `↓` `↑` | next / previous chat |
| `]` `[` / `→` `←` | next / previous directory |
| `gg` `ge` / `Home` `End` / `G` | first / last chat |
| `gn` `gp` | next / previous directory |
| `Enter` `l` `i` `a` | open: type into a live chat, or resume it |
| `n` / `Ctrl-n` | new chat (write the first message, `Tab` picks the directory) |
| `o` | new chat in this directory, no prompt |
| `O` | new chat, pick the directory first |
| `d` | kill the live instance (transcript kept) |
| `F` | fork: continue a copy of this chat as a new session |
| `p` | pin / unpin (pinned chats sit at the top) |
| `x` | archive / unarchive (hides it; transcript untouched) |
| `R` | rename: give the chat a name of your own (only cdeck sees it) |
| `E` | open in its own terminal window |
| `z` | expand / fold a directory |
| `Tab` `Shift-Tab` | cycle through live chats |
| `/` | search (see below) |
| `Esc` | clear the search filter |
| `r` | rescan transcripts |
| `M` | mouse capture off / on |
| `Ctrl-l` | redraw the whole screen (if it ever looks garbled) |
| `:` | command line |
| `space` | menu |
| `?` | show / hide the key map strip (`space ?` for full help) |

`R` opens the command line prefilled with `rename <current title>` to edit.
A local name replaces the title everywhere in cdeck (list, pane, notifications,
search) and is stored in cdeck's own state; `~/.claude` is never written.

### Focus

| Key | Action |
|-----|--------|
| `Ctrl-→` `Ctrl-←` | focus pane / chat list |
| `Ctrl-w l` `Ctrl-w h` / `g→` `g←` | same, helix window style |
| `Ctrl-w w` | swap focus |

### Pane focused (transcript preview or live session, not typing)

| Key | Action |
|-----|--------|
| `j` `k` / `↓` `↑` | scroll 3 lines (`Shift`/`Ctrl`: half page) |
| `Ctrl-d` `Ctrl-u` | scroll half a page |
| `PageDown` `PageUp` | scroll a page |
| `Home` `End` | top / bottom |
| `n` `N` | next / previous search match (when the chat matched a search) |
| `Enter` | start typing into Claude |
| `←` `Esc` | back to the chat list |

### Typing into Claude

| Key | Action |
|-----|--------|
| `Ctrl-\` | stop typing, back to the chat list |
| everything else | goes to Claude (`Esc` interrupts it, `Shift-Enter` is a newline) |

`Ctrl-\` was chosen because Claude Code never binds it, so nothing is lost.
(`Ctrl-]` is Claude Code's "open artifact" key and passes through.)

### New chat screen

| Key | Action |
|-----|--------|
| `Enter` | start the chat |
| `Shift-Enter` / `Alt-Enter` / `Ctrl-j` | newline |
| `Tab` | pick the directory (fuzzy; `Tab`/`Shift-Tab` descend/go up inside the picker) |
| `Ctrl-u` / `Ctrl-w` | clear / delete a word |
| `Esc` / `Ctrl-\` | back |

### Space menu

| Key | Action |
|-----|--------|
| `n` | new chat |
| `o` | new chat here, no prompt |
| `f` `/` | search |
| `k` | kill instance |
| `F` | fork chat |
| `p` | pin / unpin |
| `x` | archive / unarchive |
| `a` | show / hide archived chats |
| `E` | open in a new terminal window |
| `l` | toggle live-only view |
| `r` | rescan |
| `?` | full help |
| `q` | quit |

### Mouse

| Action | Effect |
|--------|--------|
| wheel on list | next / previous chat |
| click | select; click the selected chat again to open it |
| wheel on pane | scroll |
| `Shift`-drag | select text natively (most terminals), or turn capture off with `M` |

## Search

`/` filters the list as you type. Plain text matches chat titles and
directories, and (from 3 characters) a background search through the full
text of every transcript — your messages and Claude's replies, not tool output.
Chats that only matched inside the transcript show a snippet under the title.

Selecting a matching chat scrolls its preview to the first match and highlights
every occurrence; `n` / `N` in the pane step through them. Results stay current
as chats grow.

Qualifiers narrow it down; combine them freely with text:

| Qualifier | Meaning |
|-----------|---------|
| `dir:cdeck` | only chats whose directory contains `cdeck` |
| `age:<7d` | newer than 7 days (units `m` `h` `d` `w`) |
| `age:>2w` | older than 2 weeks |

Example: `/borrow dir:rust age:<1w`. `Enter` keeps the filter, `Esc` clears it.

## Commands

| Command | Action |
|---------|--------|
| `:new [dir]` / `:n` | new chat; `dir` can be fuzzy (`:new cdeck`), `Tab` completes paths |
| `:open [dir]` / `:o` | start Claude there with no prompt (fuzzy too) |
| `:kill` / `:k` | kill the selected live instance |
| `:resume` / `:resume!` | resume (`!`: even if it's running elsewhere) |
| `:fork` | fork the selected chat |
| `:pin` | pin / unpin the selected chat |
| `:archive` | archive / unarchive the selected chat |
| `:archived` | show / hide archived chats |
| `:rename [name]` | name the selected chat locally; no name goes back to Claude's title |
| `:win` / `:win!` | open in its own terminal window (`!`: even if it's running) |
| `:live` | toggle live-only view |
| `:notify` | desktop notifications on / off |
| `:mouse` | mouse capture on / off |
| `:refresh` / `:r` | rescan transcripts |
| `:help` / `:h` | full help |
| `:config` | show the config file's path, and whether it exists |
| `:q` / `:q!` | quit / quit and kill live instances (transcripts are kept) |

## Status glyphs

| Glyph | Meaning |
|-------|---------|
| `✻` (animated) | working |
| `◐` | waiting on you (permission / input) |
| `●` | idle in cdeck; **bold title** = finished while you weren't looking |
| `◆` | running in another terminal |
| `★` | pinned section |
| `×` | archived (only listed under `:archived`) |

## Notifications

When a chat running inside cdeck finishes a turn or starts waiting for
permission/input while you aren't watching it, cdeck sends a desktop
notification with `notify-send` (works with mako, dunst, swaync, …). A chat
counts as watched when it's selected and the pane has focus. `:notify` toggles
them; the footer shows `quiet` while they're off.

## Own terminal window (`E`)

Opens `claude --resume <id>` in the chat's directory in a new terminal window.
Uses `terminal` from the [config](#config) if set, then `$TERMINAL`, otherwise the first of `kitty`, `foot`, `alacritty`,
`wezterm`, `ghostty`, `xterm` found on `$PATH`.

## Config

Optional, at `$XDG_CONFIG_HOME/cdeck/config.toml` (default
`~/.config/cdeck/config.toml`). Read once at startup; every key is optional and
unknown keys are ignored. If the file doesn't parse, cdeck says so in the footer
and uses the defaults. `:config` shows where it looks.

```toml
# Below this many columns, show either the chat list or the chat, not both.
narrow_width = 100

# Chats listed per directory before the rest fold behind "… n more" (z).
group_limit = 5

# Terminal for E / :win, with any flags; overrides $TERMINAL.
terminal = "kitty --single-instance"

# Fixed chat-list width in columns. Unset: a quarter of the window, 30–48.
sidebar_width = 40
```

## Files and environment

| Path / variable | Purpose |
|-----------------|---------|
| `$CLAUDE_CONFIG_DIR` (default `~/.claude`) | where transcripts and session status are read from — never written |
| `$XDG_CONFIG_HOME/cdeck/config.toml` (default `~/.config/cdeck/`) | settings, see [Config](#config) |
| `$XDG_STATE_HOME/cdeck/` (default `~/.local/state/cdeck/`) | `pinned`, `archived` (one session id per line), `names` (`id<TAB>name` per line), and flag files `keymap-hidden`, `notify-off`, `mouse-off` |
| `$XDG_CACHE_HOME/cdeck/sessions.json` (default `~/.cache/cdeck/`) | parsed-title cache so startup doesn't reread every transcript; safe to delete |
| `$TERMINAL` | terminal used by `E` / `:win`, unless the config sets `terminal` |
| `$CDECK_CLAUDE` | program to run instead of `claude` (testing) |

## Development

```sh
cargo build
cargo test
cargo test --release -- --ignored --nocapture   # timing tests against your real history
```
