# CLAUDE.md

Created by AI Sloperator (https://www.aisloperator.com/) with Claude Code, 2026-07-21.

This file guides Claude Code (claude.ai/code) when working in this repository.

## What this is

`iren` is a Rust CLI that interactively renames files, editing each
filename readline-style directly on the terminal. It takes multiple
filenames as arguments, prints one line per file up front, and lets the
user move Enter-by-Enter through not-yet-renamed files or jump between
them with Up/Down.

## Hard constraint: minimal dependencies

The whole point of this project is to implement readline-like terminal
editing *without* pulling in a line-editing crate (no `rustyline`, no
`crossterm`, no `termion`) and without a curses/ncurses binding. Only
`std` and `libc` are permitted as dependencies (see `Cargo.toml`). Do
not add a new crate to solve a terminal-handling or CLI-parsing problem
here — implement it directly against `libc` (termios, `poll`,
`sigaction`, `ioctl`) instead. This is a deliberate design choice, not
an oversight.

Only Linux and BSD targets need to work; no Windows/console-API support
is needed.

## Architecture

- `src/term.rs` — raw termios mode (`RawGuard`), signal handling
  (`SIGINT`/`SIGTERM`/`SIGQUIT`/`SIGWINCH`/`SIGTSTP` via `sigaction`,
  with the stop/resume dance for Ctrl-Z), and vt102/CSI escape-sequence
  parsing (arrow keys including Up/Down, Home/End, Delete) using a short
  `poll(2)` timeout to distinguish a bare Escape from a longer sequence.
  Also home to UTF-8 continuation-byte reading for stdin.
- `src/editor.rs` — the single-line, readline-like editor: cursor
  motion, kill/delete operations, Escape-to-revert, and the redraw
  logic. `redraw` repaints the current line in place (`\r`, `ESC[K`,
  `ESC[nD`); `render_static` paints a line with fixed, non-editable text
  (a finalized entry's result, or the inline overwrite-confirm prompt);
  `move_rows` moves the cursor vertically between file lines (`ESC[nA`
  / `ESC[nB`). None of these ever clear the screen. Up/Down arrow
  keypresses surface from `edit_line` as `EditResult::NavigateUp`/
  `NavigateDown` carrying the in-progress buffer — `editor.rs` has no
  idea which other file line to jump to, since it doesn't know about
  the rest of the file list; that decision belongs to `main.rs`.
- `src/main.rs` — argument parsing, and the whole multi-file session in
  `run()`: prints one line per file up front (`line_text`), tracks each
  file's `LineState` (`Pending(Vec<char>)` while editable, `Done(String)`
  once finalized), and on Up/Down/Enter/Ctrl-D-skip decides which file
  line to move to next (`find_prev_pending` / `find_next_pending`, which
  search cyclically and skip `Done` entries), calling `editor::move_rows`
  to get there. Also handles the missing-file check, the y/n overwrite
  prompt, and `std::fs::rename` itself.

The one-line-per-file, fixed-line-count invariant is what makes Up/Down
navigation tractable without a curses-style full-screen redraw: every
file's line, once printed, never changes row position for the rest of
the session (even after being finalized, its `Done` text is repainted
*in place* on that same row via `render_static`), so the vertical
distance between any two files is simply the difference of their
indices, and `ESC[nA`/`ESC[nB` gets the cursor there directly. Preserve
this invariant if you touch `run()` — don't insert or remove printed
lines mid-session.

That invariant only holds if every file's line is still on screen,
though: `ESC[nA` (cursor-up) is clamped at row 1 by every real
terminal, so if printing N lines up front scrolled the earliest ones
into scrollback, a later "move up to row 0" instead lands on whatever
row is now topmost and overwrites it -- `current` stays correct
internally (so no file is ever renamed wrong), but the display becomes
actively misleading. `main()` guards against this before printing
anything, via `term::terminal_rows` (`TIOCGWINSZ`): if the file count
is `>=` the terminal's row count, it refuses to start rather than let
the display corrupt itself. This was verified empirically with `tmux`
(a real terminal emulator that enforces cursor-motion clamping, unlike
a bare pty) at `-y 6` with 10 files: without the guard, row 0's line
silently overwrote row 6's; with it, `iren` exits 1 with an explanation
before entering raw mode. The check is skipped if the row count can't
be determined (e.g. plain ptys in test harnesses, which normally never
have `TIOCSWINSZ` called on them, report `ws_row == 0`).

Signals are handled with `AtomicBool` flags set from `extern "C"`
handlers and polled after `EINTR` from blocking `read`/`poll` calls
(handlers avoid doing anything not async-signal-safe). Fatal signals
restore the original termios (stashed in a `OnceLock` at startup, since
the handler has no access to the `RawGuard` on `main`'s stack) before
exiting.

## Build / lint

```
cargo build
cargo clippy --all-targets
```

Both should be warning-free; keep them that way.

## Testing

This is an interactive terminal program, so `cargo test` alone won't
exercise the editing behavior meaningfully — stdin/stdout need to be a
real (or pseudo) TTY, and `iren` refuses to run otherwise. To verify
behavior end-to-end, drive the release binary through a pty and send
literal key sequences, e.g. with Python:

```python
import os, pty
pid, fd = pty.fork()
if pid == 0:
    os.execv("./target/release/iren", ["iren", "somefile.txt"])
else:
    os.write(fd, b"\x01\x0b")   # Ctrl-A, Ctrl-K: clear the name
    os.write(fd, b"newname.txt\r")
    ...
```

Check both the transcript (for correct escape sequences / prompts) and
the resulting filenames on disk. This is how the implementation has
been validated: renames, Escape-revert, Ctrl-D skip-on-empty, overwrite
confirm/decline, Ctrl-W word deletion, UTF-8 filenames, Ctrl-C aborting
mid-session (checking the exit code and that untouched files stay
untouched), and Up/Down navigation (send `\x1b[A`/`\x1b[B`; verify the
transcript's `ESC[nA`/`ESC[nB` deltas match the expected row distance,
including wraparound once some files are already finalized) were all
exercised this way.

Note the row-movement approach only works within one terminal screen
(see the README limitations section) — a pty test with a very long
file list won't necessarily reflect real-terminal scrolling behavior.

## Conventions

- No comments explaining *what* code does; comments (sparingly) explain
  non-obvious *why* — e.g. why `ISIG` stays enabled, why a signal
  handler only sets a flag, why `c_oflag` is left untouched.
- Keep the three-module split (`term` / `editor` / `main`); don't
  collapse terminal-control code into `main.rs` or spread ANSI-sequence
  literals across multiple files.
