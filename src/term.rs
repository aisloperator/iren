//! Raw-mode terminal control and Unix signal handling, built directly on
//! libc (termios, poll, sigaction). No line-editing library is used here;
//! `editor.rs` interprets the raw bytes and vt102/ANSI (CSI) escape
//! sequences itself.

use std::io;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

/// The original termios, stashed here so a fatal-signal handler path can
/// restore it exactly even though it has no access to the `RawGuard`
/// living on main's stack.
static ORIG_TERMIOS: OnceLock<libc::termios> = OnceLock::new();

static SIGINT_FLAG: AtomicBool = AtomicBool::new(false);
static SIGTERM_FLAG: AtomicBool = AtomicBool::new(false);
static SIGQUIT_FLAG: AtomicBool = AtomicBool::new(false);
static SIGWINCH_FLAG: AtomicBool = AtomicBool::new(false);
static SIGTSTP_FLAG: AtomicBool = AtomicBool::new(false);

extern "C" fn on_sigint(_: libc::c_int) {
    SIGINT_FLAG.store(true, Ordering::SeqCst);
}
extern "C" fn on_sigterm(_: libc::c_int) {
    SIGTERM_FLAG.store(true, Ordering::SeqCst);
}
extern "C" fn on_sigquit(_: libc::c_int) {
    SIGQUIT_FLAG.store(true, Ordering::SeqCst);
}
extern "C" fn on_sigwinch(_: libc::c_int) {
    SIGWINCH_FLAG.store(true, Ordering::SeqCst);
}
extern "C" fn on_sigtstp(_: libc::c_int) {
    SIGTSTP_FLAG.store(true, Ordering::SeqCst);
}

unsafe fn install(sig: libc::c_int, handler: extern "C" fn(libc::c_int)) {
    let mut sa: libc::sigaction = std::mem::zeroed();
    sa.sa_sigaction = handler as usize;
    libc::sigemptyset(&mut sa.sa_mask);
    // Deliberately no SA_RESTART: blocking read(2)/poll(2) calls must
    // return EINTR so the editor's event loop notices the signal.
    sa.sa_flags = 0;
    libc::sigaction(sig, &sa, std::ptr::null_mut());
}

/// Install handlers for all signals iren reacts to. Must be called once,
/// before entering raw mode.
pub fn install_signal_handlers() {
    unsafe {
        install(libc::SIGINT, on_sigint);
        install(libc::SIGTERM, on_sigterm);
        install(libc::SIGQUIT, on_sigquit);
        install(libc::SIGWINCH, on_sigwinch);
        install(libc::SIGTSTP, on_sigtstp);
    }
}

/// Saves the original termios state on construction and restores it on
/// drop, so a crash or early exit never leaves the user's shell stuck in
/// raw mode.
pub struct RawGuard {
    orig: libc::termios,
    raw: libc::termios,
}

impl RawGuard {
    pub fn new() -> io::Result<Self> {
        unsafe {
            let mut orig = MaybeUninit::<libc::termios>::uninit();
            if libc::tcgetattr(libc::STDIN_FILENO, orig.as_mut_ptr()) != 0 {
                return Err(io::Error::last_os_error());
            }
            let orig = orig.assume_init();
            let _ = ORIG_TERMIOS.set(orig);
            let mut raw = orig;
            raw.c_iflag &= !(libc::IXON | libc::ICRNL | libc::BRKINT | libc::INPCK | libc::ISTRIP);
            // c_oflag is left untouched: ONLCR keeps translating '\n' to
            // "\r\n" on output, so ordinary println!/writeln! keep working.
            raw.c_lflag &= !(libc::ICANON | libc::ECHO | libc::IEXTEN);
            // ISIG stays enabled: Ctrl-C/Ctrl-Z/Ctrl-\ raise real signals,
            // which we catch above, rather than arriving as plain bytes.
            raw.c_cc[libc::VMIN] = 1;
            raw.c_cc[libc::VTIME] = 0;
            let mut guard = RawGuard { orig, raw };
            guard.enable()?;
            Ok(guard)
        }
    }

    pub fn enable(&mut self) -> io::Result<()> {
        if unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &self.raw) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub fn restore(&self) {
        unsafe {
            libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &self.orig);
        }
    }
}

impl Drop for RawGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

/// A single input event: either a raw byte from the terminal, or a
/// notification that something happened (window resize, or resumption
/// after a Ctrl-Z suspend) that warrants redrawing the current line.
pub enum Event {
    Byte(u8),
    Redraw,
}

fn poll_stdin(timeout_ms: i32) -> io::Result<bool> {
    loop {
        let mut fds = [libc::pollfd {
            fd: libc::STDIN_FILENO,
            events: libc::POLLIN,
            revents: 0,
        }];
        let r = unsafe { libc::poll(fds.as_mut_ptr(), 1, timeout_ms) };
        if r >= 0 {
            return Ok(r > 0 && (fds[0].revents & libc::POLLIN) != 0);
        }
        let err = io::Error::last_os_error();
        if err.kind() != io::ErrorKind::Interrupted {
            return Err(err);
        }
        if check_signals() {
            return Ok(false);
        }
        // Spurious EINTR (e.g. unrelated signal): just retry the poll.
    }
}

/// Handles any pending signal flags. Fatal signals restore the tty and
/// terminate the process directly (they never return). SIGTSTP performs
/// the stop/resume dance: drop to cooked mode, actually suspend via the
/// default disposition, then re-arm raw mode on resume. Returns `true`
/// (asking the caller to redraw) for SIGWINCH and a resumed SIGTSTP.
fn check_signals() -> bool {
    if SIGINT_FLAG.swap(false, Ordering::SeqCst) {
        fatal_exit(130);
    }
    if SIGTERM_FLAG.swap(false, Ordering::SeqCst) {
        fatal_exit(143);
    }
    if SIGQUIT_FLAG.swap(false, Ordering::SeqCst) {
        fatal_exit(131);
    }
    if SIGTSTP_FLAG.swap(false, Ordering::SeqCst) {
        unsafe {
            // Drop to cooked mode so the shell we're suspended into sees a
            // sane tty, then actually stop the process ourselves --
            // installing a handler suppresses the kernel's automatic stop,
            // so we restore the default disposition and re-raise it.
            if let Some(orig) = ORIG_TERMIOS.get() {
                libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, orig);
            }
            libc::signal(libc::SIGTSTP, libc::SIG_DFL);
            libc::raise(libc::SIGTSTP);
            // Execution resumes here after a subsequent SIGCONT.
            install(libc::SIGTSTP, on_sigtstp);
        }
        // The caller re-enables raw mode via RawGuard::enable() before
        // acting on the redraw request.
        return true;
    }
    if SIGWINCH_FLAG.swap(false, Ordering::SeqCst) {
        return true;
    }
    false
}

fn fatal_exit(code: i32) -> ! {
    // The RawGuard living in main() won't get a chance to Drop from here
    // (we're exiting straight out of a signal-triggered EINTR path), so
    // restore the exact original termios stashed at startup.
    if let Some(orig) = ORIG_TERMIOS.get() {
        unsafe {
            libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, orig);
        }
    }
    println!();
    use std::io::Write;
    let _ = io::stdout().flush();
    std::process::exit(code);
}

/// Blocking read of the next raw byte from stdin, transparently handling
/// EINTR from our signal handlers (fatal signals exit the process; window
/// resizes and Ctrl-Z resumption surface as `Event::Redraw`).
pub fn read_event() -> io::Result<Event> {
    loop {
        let mut buf = [0u8; 1];
        let n = unsafe { libc::read(libc::STDIN_FILENO, buf.as_mut_ptr() as *mut libc::c_void, 1) };
        if n == 1 {
            return Ok(Event::Byte(buf[0]));
        }
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "stdin closed"));
        }
        let err = io::Error::last_os_error();
        if err.kind() != io::ErrorKind::Interrupted {
            return Err(err);
        }
        if check_signals() {
            return Ok(Event::Redraw);
        }
        // Spurious EINTR: loop back and read again.
    }
}

/// The decoded meaning of a vt102/xterm-style escape sequence following
/// a lone ESC (0x1b) byte.
pub enum Key {
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    Delete,
    /// A bare ESC with nothing following within the timeout window.
    Bare,
    Unknown,
}

/// Parses a CSI (ESC [ ...) or SS3 (ESC O ...) sequence after the leading
/// ESC byte has already been consumed. Uses a short poll(2) timeout to
/// distinguish a standalone Escape keypress from the start of a
/// multi-byte arrow/Home/End/Delete sequence, since both begin with the
/// same 0x1b byte.
pub fn parse_escape() -> io::Result<Key> {
    if !poll_stdin(50)? {
        return Ok(Key::Bare);
    }
    let b1 = match read_event()? {
        Event::Byte(b) => b,
        Event::Redraw => return Ok(Key::Unknown),
    };
    match b1 {
        b'[' => parse_csi(),
        b'O' => {
            if !poll_stdin(50)? {
                return Ok(Key::Unknown);
            }
            match read_event()? {
                Event::Byte(b'H') => Ok(Key::Home),
                Event::Byte(b'F') => Ok(Key::End),
                _ => Ok(Key::Unknown),
            }
        }
        _ => Ok(Key::Unknown),
    }
}

fn parse_csi() -> io::Result<Key> {
    if !poll_stdin(50)? {
        return Ok(Key::Unknown);
    }
    let b2 = match read_event()? {
        Event::Byte(b) => b,
        Event::Redraw => return Ok(Key::Unknown),
    };
    match b2 {
        b'A' => Ok(Key::Up),
        b'B' => Ok(Key::Down),
        b'C' => Ok(Key::Right),
        b'D' => Ok(Key::Left),
        b'H' => Ok(Key::Home),
        b'F' => Ok(Key::End),
        b'0'..=b'9' => {
            let mut num = b2 - b'0';
            loop {
                if !poll_stdin(50)? {
                    return Ok(Key::Unknown);
                }
                match read_event()? {
                    Event::Byte(b'~') => {
                        return Ok(match num {
                            1 | 7 => Key::Home,
                            3 => Key::Delete,
                            4 | 8 => Key::End,
                            _ => Key::Unknown,
                        });
                    }
                    Event::Byte(d) if d.is_ascii_digit() => {
                        num = num.saturating_mul(10).saturating_add(d - b'0');
                    }
                    _ => return Ok(Key::Unknown),
                }
            }
        }
        _ => Ok(Key::Unknown),
    }
}

/// Reads any UTF-8 continuation bytes needed to complete the character
/// that started with `first`, returning `None` for invalid/unsupported
/// lead bytes (which are then simply ignored by the editor).
pub fn read_utf8_char(first: u8) -> io::Result<Option<char>> {
    let extra = if first < 0x80 {
        0
    } else if first & 0xE0 == 0xC0 {
        1
    } else if first & 0xF0 == 0xE0 {
        2
    } else if first & 0xF8 == 0xF0 {
        3
    } else {
        return Ok(None);
    };
    let mut bytes = vec![first];
    for _ in 0..extra {
        match read_event()? {
            Event::Byte(b) => bytes.push(b),
            Event::Redraw => return Ok(None),
        }
    }
    Ok(std::str::from_utf8(&bytes).ok().and_then(|s| s.chars().next()))
}

pub fn is_tty(fd: i32) -> bool {
    unsafe { libc::isatty(fd) == 1 }
}
