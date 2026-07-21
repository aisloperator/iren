//! A minimal, readline-like single-line editor. Talks to the terminal
//! purely through vt102/ANSI CSI escape sequences (cursor-left/right/
//! up/down, erase-to-end-of-line) issued over stdout, and raw bytes read
//! from stdin via `term::read_event`. No screen clearing: each file gets
//! exactly one terminal line for its whole lifetime, and Up/Down move
//! the cursor between those lines with plain vt102 cursor-motion
//! sequences rather than repainting the screen.

use crate::term::{self, Event, Key, RawGuard};
use std::io::{self, Write};

pub enum EditResult {
    Confirmed(String),
    /// User asked to leave this entry alone (Ctrl-D on an empty line).
    Skipped,
    /// Up/Down arrow pressed; carries the buffer as last edited so the
    /// caller can preserve it if it navigates elsewhere.
    NavigateUp(String),
    NavigateDown(String),
}

/// Runs interactive line editing for one filename. `prefix` is a fixed,
/// non-editable label printed before the editable text (e.g. "(1/3) ").
/// `seed` is the text to start editing from (a prior in-progress edit,
/// if the user navigated back to this entry, or the original name for a
/// fresh entry); `original` is what a bare Escape keypress reverts to.
pub fn edit_line(
    out: &mut impl Write,
    raw: &mut RawGuard,
    prefix: &str,
    seed: &str,
    original: &str,
) -> io::Result<EditResult> {
    let mut buf: Vec<char> = seed.chars().collect();
    let mut cursor = buf.len();

    redraw(out, prefix, &buf, cursor)?;
    loop {
        match term::read_event()? {
            Event::Redraw => {
                raw.enable()?;
                redraw(out, prefix, &buf, cursor)?;
            }
            Event::Byte(b) => {
                match b {
                    b'\r' | b'\n' => {
                        return Ok(EditResult::Confirmed(buf.into_iter().collect()));
                    }
                    0x7f | 0x08 => {
                        // Backspace (DEL or BS).
                        if cursor > 0 {
                            buf.remove(cursor - 1);
                            cursor -= 1;
                        }
                    }
                    0x04 => {
                        // Ctrl-D: delete-forward, or skip if line is empty.
                        if buf.is_empty() {
                            return Ok(EditResult::Skipped);
                        }
                        if cursor < buf.len() {
                            buf.remove(cursor);
                        }
                    }
                    0x01 => cursor = 0,             // Ctrl-A: home
                    0x05 => cursor = buf.len(),      // Ctrl-E: end
                    0x02 => cursor = cursor.saturating_sub(1), // Ctrl-B: left
                    0x06 => cursor = (cursor + 1).min(buf.len()), // Ctrl-F: right
                    0x0b => buf.truncate(cursor),     // Ctrl-K: kill to end
                    0x15 => {
                        // Ctrl-U: kill to start.
                        buf.drain(0..cursor);
                        cursor = 0;
                    }
                    0x17 => kill_word_back(&mut buf, &mut cursor), // Ctrl-W
                    0x1b => match term::parse_escape()? {
                        Key::Left => cursor = cursor.saturating_sub(1),
                        Key::Right => cursor = (cursor + 1).min(buf.len()),
                        Key::Up => return Ok(EditResult::NavigateUp(buf.into_iter().collect())),
                        Key::Down => return Ok(EditResult::NavigateDown(buf.into_iter().collect())),
                        Key::Home => cursor = 0,
                        Key::End => cursor = buf.len(),
                        Key::Delete => {
                            if cursor < buf.len() {
                                buf.remove(cursor);
                            }
                        }
                        Key::Bare => {
                            // Standalone Escape: revert to the original text.
                            buf = original.chars().collect();
                            cursor = buf.len();
                        }
                        Key::Unknown => bell(out)?,
                    },
                    0x00..=0x1f => bell(out)?, // other unhandled control chars
                    _ => {
                        if let Some(c) = term::read_utf8_char(b)? {
                            buf.insert(cursor, c);
                            cursor += 1;
                        }
                    }
                }
                redraw(out, prefix, &buf, cursor)?;
            }
        }
    }
}

/// Deletes the word immediately before the cursor (Ctrl-W): trailing
/// whitespace first, then the run of non-whitespace before it.
fn kill_word_back(buf: &mut Vec<char>, cursor: &mut usize) {
    let mut start = *cursor;
    while start > 0 && buf[start - 1].is_whitespace() {
        start -= 1;
    }
    while start > 0 && !buf[start - 1].is_whitespace() {
        start -= 1;
    }
    buf.drain(start..*cursor);
    *cursor = start;
}

pub fn bell(out: &mut impl Write) -> io::Result<()> {
    write!(out, "\x07")?;
    out.flush()
}

/// Repaints the current line in place: return to column 0, print the
/// fixed prefix and the live buffer, erase any leftover tail from a
/// previously longer buffer, then move the cursor back to its logical
/// position. Uses only vt102-standard CSI sequences (ESC[K, ESC[nD).
fn redraw(out: &mut impl Write, prefix: &str, buf: &[char], cursor: usize) -> io::Result<()> {
    write!(out, "\r")?;
    write!(out, "{}", prefix)?;
    for c in buf {
        write!(out, "{}", c)?;
    }
    write!(out, "\x1b[K")?; // erase from cursor to end of line
    let back = buf.len() - cursor;
    if back > 0 {
        write!(out, "\x1b[{}D", back)?; // cursor left `back` columns
    }
    out.flush()
}

/// Repaints the current line in place with fixed, non-editable text (used
/// for a finalized entry's result, and for the inline overwrite-confirm
/// prompt). The cursor is simply left at the end of `text`.
pub fn render_static(out: &mut impl Write, text: &str) -> io::Result<()> {
    write!(out, "\r{text}\x1b[K")?;
    out.flush()
}

/// Moves the cursor vertically by `delta` rows (negative = up, positive =
/// down) using the standard vt102 CUU/CUD sequences. A no-op for 0.
pub fn move_rows(out: &mut impl Write, delta: isize) -> io::Result<()> {
    match delta.cmp(&0) {
        std::cmp::Ordering::Less => write!(out, "\x1b[{}A", -delta)?,
        std::cmp::Ordering::Greater => write!(out, "\x1b[{delta}B")?,
        std::cmp::Ordering::Equal => {}
    }
    out.flush()
}
