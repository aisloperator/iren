# CLAUDE.md

Created by AI Sloperator (https://www.aisloperator.com/) with Claude Code, 2026-07-21.

This file guides Claude Code (claude.ai/code) when working in this repository.

## What this is

`iren` is a Rust CLI that interactively renames files, editing each
filename readline-style directly on the terminal. It takes multiple
filenames as arguments and processes them one at a time, moving to the
next file when the user presses Enter.

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
  parsing (arrow keys, Home/End, Delete) using a short `poll(2)`
  timeout to distinguish a bare Escape from a longer sequence. Also
  home to UTF-8 continuation-byte reading for stdin.
- `src/editor.rs` — the single-line, readline-like editor: cursor
  motion, kill/delete operations, Escape-to-revert, redraw logic. The
  redraw function is the only place that emits ANSI control sequences
  for repainting (`\r`, `ESC[K`, `ESC[nD`); it never clears the screen.
- `src/main.rs` — argument parsing, per-file orchestration (skip
  missing files, confirm before overwriting an existing target, call
  `std::fs::rename`), and the y/n overwrite prompt.

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
the resulting filenames on disk. This is how the initial implementation
was validated: renames, Escape-revert, Ctrl-D skip-on-empty, overwrite
confirm/decline, Ctrl-W word deletion, UTF-8 filenames, and Ctrl-C
aborting mid-session (checking the exit code and that untouched files
stay untouched) were all exercised this way.

## Conventions

- No comments explaining *what* code does; comments (sparingly) explain
  non-obvious *why* — e.g. why `ISIG` stays enabled, why a signal
  handler only sets a flag, why `c_oflag` is left untouched.
- Keep the three-module split (`term` / `editor` / `main`); don't
  collapse terminal-control code into `main.rs` or spread ANSI-sequence
  literals across multiple files.
