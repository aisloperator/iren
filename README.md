# iren

*Created by AI Sloperator ([www.aisloperator.com](https://www.aisloperator.com/)) with Claude Code, 2026-07-21.*

`iren` is a small command-line tool for interactively renaming files. For
each filename given on the command line, it lets you edit the name
readline-style, right on the terminal, then presses on to the next
not-yet-renamed file when you hit Enter.

## Usage

```
iren FILE...
```

Every file is printed up front, one per line, with its current name
pre-filled and ready to edit:

```
(1/3) IMG_0001.JPG
(2/3) IMG_0002.JPG
(3/3) IMG_0003.JPG
```

Edit the highlighted line and press Enter to rename it and move on to
the next file that hasn't been renamed yet.

### Keys

| Keys                     | Action                                   |
|---------------------------|-------------------------------------------|
| Left / Right, Ctrl-B / Ctrl-F | Move the cursor                        |
| Home / End, Ctrl-A / Ctrl-E   | Jump to start / end of the name        |
| Up / Down                | Switch to another not-yet-renamed file, including one skipped this way earlier |
| Backspace, Delete, Ctrl-D | Delete a character                        |
| Ctrl-W                    | Delete the previous word                  |
| Ctrl-U / Ctrl-K           | Kill to start / end of the line           |
| Escape                    | Revert to the original name               |
| Ctrl-D on an empty line   | Leave this file unchanged for good, skip to next |
| Enter                     | Confirm and move to the next not-yet-renamed file |
| Ctrl-C                    | Abort; remaining files are left untouched |

Up/Down move between files that haven't been finalized yet (edited-but-
not-confirmed, or never visited); once a file is renamed, left
unchanged via Ctrl-D, or confirmed unchanged via Enter, it's locked in
and no longer reachable by Up/Down. If there's nowhere left to go, the
terminal bell rings instead.

If the edited name matches an existing file, `iren` asks for
confirmation before overwriting it; declining lets you keep editing the
same name.

## Design

`iren` depends on nothing but the Rust standard library and `libc`.
There is no readline/rustyline dependency and no curses/ncurses
dependency. Terminal editing is implemented directly:

- Raw terminal mode is set with `termios` via `libc`.
- Every file gets exactly one terminal line, printed once up front.
  Editing redraws that line in place with plain vt102/ANSI control
  sequences (`\r`, `ESC[K`, `ESC[nD`); Up/Down move the cursor to a
  different file's line with `ESC[nA` / `ESC[nB` (cursor up/down). The
  screen is never cleared, and the total line count never changes —
  just the standard cursor-motion sequences a vt102 already understands.
- Arrow keys, Home/End, and Delete are recognized by parsing their CSI
  escape sequences by hand, using a short `poll(2)` timeout to tell a
  bare Escape keypress apart from the start of a longer sequence.
- `SIGINT`, `SIGTERM`, `SIGQUIT`, `SIGWINCH`, and `SIGTSTP` are handled
  directly with `sigaction`, including the stop/resume dance needed to
  make Ctrl-Z suspend the process properly and restore raw mode on
  resume. The original terminal settings are always restored before
  exiting, including on a fatal signal.

This only targets Linux and BSD-family systems, and requires stdin and
stdout to be an interactive terminal.

## Building

```
cargo build --release
```

The binary is written to `target/release/iren`.

## Limitations

- Filenames wider than the terminal will not wrap or scroll correctly;
  the editor assumes the prompt and name fit on one visible line.
- Cursor math counts Unicode scalar values, not display width, so
  wide/combining characters may misalign the cursor slightly.
- Up/Down navigation relies on cursor-relative vertical movement within
  a single terminal screen. If the file list is longer than the
  terminal's visible height, lines that have scrolled off the top may
  not be reachable this way.
