//! iren: interactively rename files, editing each filename on the
//! terminal readline-style. Built only on the Rust standard library and
//! libc -- terminal raw mode, ANSI/vt102 escape sequences, and Unix
//! signal handling are all implemented directly in `term`/`editor`
//! rather than pulling in an off-the-shelf line-editing crate.
//!
//! Every file gets exactly one terminal line, printed up front; Up/Down
//! move the edit cursor between not-yet-renamed lines with vt102
//! cursor-motion sequences (never a screen clear), so you can jump ahead
//! or back over files you haven't confirmed yet.

mod editor;
mod term;

use std::io::{self, Write};
use std::path::Path;

fn print_usage(prog: &str) {
    eprintln!("usage: {prog} FILE...");
    eprintln!();
    eprintln!("Interactively rename each FILE. The current name is pre-filled and");
    eprintln!("editable readline-style; press Enter to confirm the rename and move");
    eprintln!("to the next not-yet-renamed file.");
    eprintln!();
    eprintln!("keys: Left/Right, Ctrl-B/F        move cursor");
    eprintln!("      Home/End, Ctrl-A/E          jump to start/end");
    eprintln!("      Up/Down                     switch to another not-yet-renamed");
    eprintln!("                                  file (including ones skipped this way)");
    eprintln!("      Backspace, Delete, Ctrl-D   delete a character");
    eprintln!("      Ctrl-W                      delete previous word");
    eprintln!("      Ctrl-U/K                    kill to start/end of line");
    eprintln!("      Escape                      revert to the original name");
    eprintln!("      Ctrl-D on an empty line      leave this file unchanged");
    eprintln!("      Enter                       confirm and move to next file");
    eprintln!("      Ctrl-C                      abort, leaving remaining files untouched");
}

/// The state of one file's line: still editable, or finalized with fixed
/// display text (renamed, left unchanged, not found, etc).
enum LineState {
    Pending(Vec<char>),
    Done(String),
}

struct FileEntry {
    original: String,
    state: LineState,
}

fn main() {
    let mut args = std::env::args();
    let prog = args.next().unwrap_or_else(|| "iren".to_string());
    let files: Vec<String> = args.collect();

    if files.is_empty() {
        print_usage(&prog);
        std::process::exit(1);
    }
    if files.iter().any(|a| a == "-h" || a == "--help") {
        print_usage(&prog);
        std::process::exit(0);
    }

    if !term::is_tty(libc::STDIN_FILENO) || !term::is_tty(libc::STDOUT_FILENO) {
        eprintln!("{prog}: stdin and stdout must be a terminal for interactive editing");
        std::process::exit(1);
    }

    let mut entries: Vec<FileEntry> = files
        .into_iter()
        .map(|orig| {
            if Path::new(&orig).symlink_metadata().is_err() {
                let state = LineState::Done(format!("{orig} (not found, skipped)"));
                FileEntry { original: orig, state }
            } else {
                let state = LineState::Pending(orig.chars().collect());
                FileEntry { original: orig, state }
            }
        })
        .collect();

    term::install_signal_handlers();
    let mut raw = match term::RawGuard::new() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("{prog}: failed to set raw terminal mode: {e}");
            std::process::exit(1);
        }
    };

    let stdout = io::stdout();
    let mut out = stdout.lock();

    if let Err(e) = run(&mut out, &mut raw, &mut entries) {
        eprintln!("{prog}: I/O error: {e}");
        drop(raw);
        std::process::exit(1);
    }

    drop(raw);
}

/// The full session: prints one line per file, then edits back and forth
/// among the not-yet-finalized ones until all are done.
fn run(out: &mut impl Write, raw: &mut term::RawGuard, entries: &mut [FileEntry]) -> io::Result<()> {
    let n = entries.len();

    // Print one line per file. The cursor ends up at column 0 of a fresh
    // line just below the last file's line -- call that virtual row `n`.
    for (i, entry) in entries.iter().enumerate() {
        writeln!(out, "{}", line_text(i, n, entry))?;
    }

    let Some(first) = find_pending_from(entries, 0) else {
        return Ok(()); // Nothing to edit; cursor is already below the list.
    };
    editor::move_rows(out, first as isize - n as isize)?;
    let mut current = first;

    loop {
        let seed: String = match &entries[current].state {
            LineState::Pending(buf) => buf.iter().collect(),
            LineState::Done(_) => unreachable!("current entry is always pending"),
        };
        let prefix = format!("({}/{}) ", current + 1, n);
        let result = editor::edit_line(out, raw, &prefix, &seed, &entries[current].original)?;

        match result {
            editor::EditResult::NavigateUp(buf) => {
                entries[current].state = LineState::Pending(buf.chars().collect());
                match find_prev_pending(entries, current) {
                    Some(target) => {
                        editor::move_rows(out, target as isize - current as isize)?;
                        current = target;
                    }
                    None => editor::bell(out)?,
                }
            }
            editor::EditResult::NavigateDown(buf) => {
                entries[current].state = LineState::Pending(buf.chars().collect());
                match find_next_pending(entries, current) {
                    Some(target) => {
                        editor::move_rows(out, target as isize - current as isize)?;
                        current = target;
                    }
                    None => editor::bell(out)?,
                }
            }
            editor::EditResult::Skipped => {
                let orig = &entries[current].original;
                entries[current].state = LineState::Done(format!("{orig} (left unchanged)"));
                editor::render_static(out, &line_text(current, n, &entries[current]))?;
                match advance(out, entries, current, n)? {
                    Some(next) => current = next,
                    None => break,
                }
            }
            editor::EditResult::Confirmed(new_name) => {
                let orig = entries[current].original.clone();
                if new_name == orig {
                    entries[current].state = LineState::Done(format!("{orig} (unchanged)"));
                } else if new_name.is_empty() {
                    entries[current].state = LineState::Done(format!("{orig} (empty name, skipped)"));
                } else if Path::new(&new_name).symlink_metadata().is_ok() {
                    let question =
                        format!("({}/{}) '{new_name}' exists, overwrite? [y/N] ", current + 1, n);
                    editor::render_static(out, &question)?;
                    let yes = confirm(out, raw)?;
                    if !yes {
                        entries[current].state = LineState::Pending(new_name.chars().collect());
                        continue; // Stay on this row, back to edit mode with the attempted name.
                    }
                    entries[current].state = finalize_rename(&orig, &new_name);
                } else {
                    entries[current].state = finalize_rename(&orig, &new_name);
                }
                editor::render_static(out, &line_text(current, n, &entries[current]))?;
                match advance(out, entries, current, n)? {
                    Some(next) => current = next,
                    None => break,
                }
            }
        }
    }

    Ok(())
}

fn finalize_rename(orig: &str, new_name: &str) -> LineState {
    match std::fs::rename(orig, new_name) {
        Ok(()) => LineState::Done(format!("{orig} -> {new_name}")),
        Err(e) => LineState::Done(format!("{orig} (rename failed: {e})")),
    }
}

/// Renders a file's full line, including its `(i/N) ` prefix.
fn line_text(i: usize, n: usize, entry: &FileEntry) -> String {
    let prefix = format!("({}/{}) ", i + 1, n);
    match &entry.state {
        LineState::Pending(buf) => format!("{prefix}{}", buf.iter().collect::<String>()),
        LineState::Done(text) => format!("{prefix}{text}"),
    }
}

fn find_pending_from(entries: &[FileEntry], start: usize) -> Option<usize> {
    (start..entries.len()).find(|&i| matches!(entries[i].state, LineState::Pending(_)))
}

/// Finds the nearest pending entry before `current`, wrapping around.
fn find_prev_pending(entries: &[FileEntry], current: usize) -> Option<usize> {
    let n = entries.len();
    (1..n)
        .map(|step| (current + n - step) % n)
        .find(|&i| matches!(entries[i].state, LineState::Pending(_)))
}

/// Finds the nearest pending entry after `current`, wrapping around.
fn find_next_pending(entries: &[FileEntry], current: usize) -> Option<usize> {
    let n = entries.len();
    (1..n)
        .map(|step| (current + step) % n)
        .find(|&i| matches!(entries[i].state, LineState::Pending(_)))
}

/// Moves on from a just-finalized entry to the next pending one, or, if
/// none remain, down to the blank line below the whole list.
fn advance(
    out: &mut impl Write,
    entries: &[FileEntry],
    current: usize,
    n: usize,
) -> io::Result<Option<usize>> {
    match find_next_pending(entries, current) {
        Some(target) => {
            editor::move_rows(out, target as isize - current as isize)?;
            Ok(Some(target))
        }
        None => {
            editor::move_rows(out, n as isize - current as isize)?;
            // move_rows is a pure cursor-up/down (CUU/CUD) move and
            // preserves the column; every other call site is immediately
            // followed by edit_line's redraw (which starts with \r), but
            // this is the final move of the session, so nothing else
            // resets the column afterwards -- do it here instead.
            write!(out, "\r")?;
            out.flush()?;
            Ok(None)
        }
    }
}

/// Reads a single y/n keypress (defaulting to "no"), tolerating the same
/// redraw-worthy events (SIGWINCH, Ctrl-Z resume) the line editor does.
fn confirm(out: &mut impl Write, raw: &mut term::RawGuard) -> io::Result<bool> {
    loop {
        match term::read_event()? {
            term::Event::Redraw => {
                raw.enable()?;
                continue;
            }
            term::Event::Byte(b) => {
                let yes = b == b'y' || b == b'Y';
                write!(out, "{}", if yes { "yes" } else { "no" })?;
                out.flush()?;
                return Ok(yes);
            }
        }
    }
}
