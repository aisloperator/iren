//! iren: interactively rename files, editing each filename on the
//! terminal readline-style. Built only on the Rust standard library and
//! libc -- terminal raw mode, ANSI/vt102 escape sequences, and Unix
//! signal handling are all implemented directly in `term`/`editor`
//! rather than pulling in an off-the-shelf line-editing crate.

mod editor;
mod term;

use std::io::{self, Write};
use std::path::Path;

fn print_usage(prog: &str) {
    eprintln!("usage: {prog} FILE...");
    eprintln!();
    eprintln!("Interactively rename each FILE, one at a time. The current name is");
    eprintln!("pre-filled and editable readline-style; press Enter to confirm the");
    eprintln!("rename and move to the next file.");
    eprintln!();
    eprintln!("keys: Left/Right, Ctrl-B/F        move cursor");
    eprintln!("      Home/End, Ctrl-A/E          jump to start/end");
    eprintln!("      Backspace, Delete, Ctrl-D   delete a character");
    eprintln!("      Ctrl-W                      delete previous word");
    eprintln!("      Ctrl-U/K                    kill to start/end of line");
    eprintln!("      Escape                      revert to the original name");
    eprintln!("      Ctrl-D on an empty line      leave this file unchanged");
    eprintln!("      Enter                       confirm and move to next file");
    eprintln!("      Ctrl-C                      abort, leaving remaining files untouched");
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
    let total = files.len();

    for (i, orig) in files.iter().enumerate() {
        if let Err(e) = process_file(&mut out, &mut raw, i + 1, total, orig) {
            eprintln!("{prog}: I/O error: {e}");
            drop(raw);
            std::process::exit(1);
        }
    }

    drop(raw);
}

fn process_file(
    out: &mut impl Write,
    raw: &mut term::RawGuard,
    index: usize,
    total: usize,
    orig: &str,
) -> io::Result<()> {
    if Path::new(orig).symlink_metadata().is_err() {
        writeln!(out, "({index}/{total}) {orig}: not found, skipping")?;
        return Ok(());
    }

    let prefix = format!("({index}/{total}) ");
    let mut current = orig.to_string();

    loop {
        let result = editor::edit_line(out, raw, &prefix, &current)?;
        writeln!(out)?; // move past the line being edited
        match result {
            editor::EditResult::Skipped => {
                writeln!(out, "  (left unchanged)")?;
                return Ok(());
            }
            editor::EditResult::Confirmed(new_name) => {
                if new_name == *orig {
                    writeln!(out, "  (unchanged)")?;
                    return Ok(());
                }
                if new_name.is_empty() {
                    writeln!(out, "  (empty name, skipped)")?;
                    return Ok(());
                }
                if Path::new(&new_name).symlink_metadata().is_ok() {
                    write!(out, "  '{new_name}' already exists, overwrite? [y/N] ")?;
                    out.flush()?;
                    let yes = confirm(out, raw)?;
                    writeln!(out)?;
                    if !yes {
                        current = new_name; // keep their edit, let them adjust it
                        continue;
                    }
                }
                match std::fs::rename(orig, &new_name) {
                    Ok(()) => writeln!(out, "  '{orig}' -> '{new_name}'")?,
                    Err(e) => writeln!(out, "  error renaming '{orig}': {e}")?,
                }
                return Ok(());
            }
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
