# iren

*Created by AI Sloperator ([www.aisloperator.com](https://www.aisloperator.com/)) with Claude Code, 2026-07-21.*

`iren` is a small command-line tool for interactively renaming files. For
each filename given on the command line, it lets you edit the name
readline-style, right on the terminal, then presses on to the next file
when you hit Enter.

## Usage

```
iren FILE...
```

Each file is presented one at a time with its current name pre-filled
and ready to edit:

```
(1/3) IMG_0001.JPG
```

Edit the name and press Enter to rename and move on to the next file.

### Keys

| Keys                     | Action                                   |
|---------------------------|-------------------------------------------|
| Left / Right, Ctrl-B / Ctrl-F | Move the cursor                        |
| Home / End, Ctrl-A / Ctrl-E   | Jump to start / end of the name        |
| Backspace, Delete, Ctrl-D | Delete a character                        |
| Ctrl-W                    | Delete the previous word                  |
| Ctrl-U / Ctrl-K           | Kill to start / end of the line           |
| Escape                    | Revert to the original name               |
| Ctrl-D on an empty line   | Leave this file unchanged, skip to next   |
| Enter                     | Confirm and move to the next file         |
| Ctrl-C                    | Abort; remaining files are left untouched |

If the edited name matches an existing file, `iren` asks for
confirmation before overwriting it; declining lets you keep editing the
same name.

## Design

`iren` depends on nothing but the Rust standard library and `libc`.
There is no readline/rustyline dependency and no curses/ncurses
dependency. Terminal editing is implemented directly:

- Raw terminal mode is set with `termios` via `libc`.
- Line redrawing uses plain vt102/ANSI control sequences (`\r`,
  `ESC[K`, `ESC[nD`) written straight to stdout — the screen is never
  cleared; each prompt simply occupies the terminal's current line, and
  finishing a file advances to a fresh line like a normal shell would.
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
