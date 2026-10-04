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
`zoxide` (more directories in the picker), a terminal emulator for `E`,
`wl-copy` or `xclip` for `y`/`Y` (otherwise the terminal is asked via OSC 52).

## Layout

The chat list (sidebar) is on the left, the pane on the right. The pane shows
the selected chat: a live Claude session if it's running in cdeck, otherwise a
read-only preview of its transcript.

Below 100 columns (`narrow_width` in the [config](#config); e.g. a half-width window in a tiling WM) only one of them is
shown at a time: whichever has focus. `Ctrl-\` from a chat brings the list
back; opening a chat shows it.

Directories with many chats show their 5 newest (`group_limit`); `z` expands them.

### Token usage

The pane's title bar shows the chat's tokens, e.g. `12.3k out · 1.2M in`
(`in` counts everything the model read, cached or not), and the footer shows
today's output across all chats. Tokens rather than dollars, since on a
subscription the bill doesn't change. Each chat counts its own transcript;
subagents keep separate transcripts (`<id>/subagents/`) and aren't included.

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
| `y` | copy Claude's last reply (works from the pane too) |
| `Y` | copy the whole chat as Markdown (`## You` / `## Claude`, tool calls as `> ⚙ …`) |
| `z` | expand / fold a directory |
| `Tab` `Shift-Tab` | cycle through live chats |
| `u` | jump to the chat that needs you (see below) |
| `A` / `D` | allow / deny the selected chat's permission prompt without opening it |
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
| `Ctrl-→` `Ctrl-←` | focus one step right / left: chat list, pane (left half, right half when split) |
| `Ctrl-w l` `Ctrl-w h` / `g→` `g←` | same, helix window style |
| `Ctrl-w w` | next: chat list → pane → right half → chat list |
| `Ctrl-w v` | split the pane: keep the selected chat on the right |
| `Ctrl-w q` `Ctrl-w o` | close the split |

### Split pane

`Ctrl-w v` shows two chats side by side: the chat selected when you pressed it
stays in the right half, and the left half keeps following the selection, so you
can watch one chat while browsing or typing in others. `Ctrl-w v` again moves
the right half to the current selection.

Each half takes focus on its own (`Ctrl-w l` / `Ctrl-w h` / `Ctrl-w w`; the
focused half's rule under the title lights up). Pane keys, `Enter`, typing,
paste and `d` act on the focused half's chat; list keys like `p`, `x`, `F`, `R`
still act on the selection. Narrow windows (below `narrow_width`)
never split; shrinking one closes the split. While split, every live session is
sized to half the pane (both halves are the same width), and back to the full
pane when the split closes.

### Pane focused (transcript preview or live session, not typing)

When split, these act on whichever half has focus.

| Key | Action |
|-----|--------|
| `j` `k` / `↓` `↑` | scroll 3 lines (`Shift`/`Ctrl`: half page) |
| `Ctrl-d` `Ctrl-u` | scroll half a page |
| `PageDown` `PageUp` | scroll a page |
| `Home` `End` | top / bottom |
| `n` `N` | next / previous search match (when the chat matched a search) |
| `y` `Y` | copy Claude's last reply / the whole chat as Markdown |
| `t` | show / hide tool output in transcript previews (first 6 lines of each result, under its call) |
| `Enter` | start typing into Claude |
| `u` | jump to the chat that needs you |
| `A` / `D` | allow / deny its permission prompt |
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
| `u` | go to the chat that needs you |
| `?` | full help |
| `q` | quit |

### Mouse

| Action | Effect |
|--------|--------|
| wheel on list | next / previous chat |
| click | select; click the selected chat again to open it |
| click on pane | focus it (that half, when split); a live chat starts typing |
| wheel on pane | scroll (each half of a split scrolls on its own) |
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
| `:bell` | terminal bell on / off (marks the window urgent) |
| `:mouse` | mouse capture on / off |
| `:tools` | tool output in transcript previews on / off, like `t` |
| `:copy` / `:copy all` | copy Claude's last reply / the whole chat, like `y` / `Y` |
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

`u` (from the list or the pane) selects the next chat that needs you: first
the ones waiting (`◐`), longest-waiting first, then the ones that finished
while you weren't looking (bold), oldest first. Pressing it again moves on to
the next one, wrapping around. Only chats shown in the list count, so a search
filter can hide them.

## Permission prompts

When the selected chat is waiting on a tool permission prompt, the footer says
what it wants (`◐ needs permission: Bash command · rm -rf target · … · A allow
· D deny`), and you can answer without opening the chat:

- `A` allows it once: cdeck presses `Enter`, but only while Claude's dialog is
  on screen with plain "Yes" highlighted, never "Yes, and don't ask again".
- `D` denies it: cdeck presses `Esc`, Claude's own "No" key.

Before sending anything cdeck checks that Claude's status file says it's
waiting on a permission prompt and that the dialog is actually drawn in the
pane; otherwise it tells you to open the chat (`Enter`) and answer there.
A second press within a moment of the first is refused, so it can't land in
whatever Claude shows next. Chats running in another terminal (`◆`) are never
touched. Questions Claude asks you (`needs you: input needed`) still need
opening.

## Notifications

When a chat running inside cdeck finishes a turn or starts waiting for
permission/input while you aren't watching it, cdeck sends a desktop
notification with `notify-send` (works with mako, dunst, swaync, …). A chat
counts as watched when it's selected and the pane has focus. `:notify` toggles
them; the footer shows `quiet` while they're off.

At the same moments cdeck also rings the terminal bell, which most terminals
turn into a window urgency hint: kitty marks the window urgent, and Hyprland
(or sway, i3, …) highlights its workspace. `:bell` toggles it independently of
`:notify`, so you can have either, both or neither; the footer shows `no bell`
while it's off.

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
| `$XDG_STATE_HOME/cdeck/` (default `~/.local/state/cdeck/`) | `pinned`, `archived` (one session id per line), `names` (`id<TAB>name` per line), and flag files `keymap-hidden`, `notify-off`, `bell-off`, `mouse-off` |
| `$XDG_CACHE_HOME/cdeck/sessions.json` (default `~/.cache/cdeck/`) | parsed titles and token usage, so startup doesn't reread every transcript; safe to delete |
| `$TERMINAL` | terminal used by `E` / `:win`, unless the config sets `terminal` |
| `$CDECK_CLAUDE` | program to run instead of `claude` (testing) |

## Development

```sh
cargo build
cargo test
cargo test --release -- --ignored --nocapture   # timing tests against your real history
```
