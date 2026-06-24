//! Lightweight terminal progress: a determinate download bar and an
//! indeterminate spinner. All output goes to stderr (so stdout stays clean for
//! piping). ANSI redraws are used only when stderr is a terminal — detected via
//! the `wasi:cli/terminal-stderr` import — otherwise plain milestone lines are
//! printed. Hand-rolled because `indicatif`/`console` rely on libc terminal
//! calls unavailable under wasi.

use std::io::Write;
use wvm_core::human_bytes;

const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const BAR_WIDTH: usize = 24;

/// True if stderr is a terminal (so ANSI redraws are appropriate).
pub fn stderr_is_terminal() -> bool {
    crate::wasi::cli::terminal_stderr::get_terminal_stderr().is_some()
}

/// True if stdout is a terminal. Used to tell whether `wvm use` output is being
/// captured by the shell hook (`eval "$(...)"`, not a terminal) versus printed
/// directly to the user (a terminal).
pub fn stdout_is_terminal() -> bool {
    crate::wasi::cli::terminal_stdout::get_terminal_stdout().is_some()
}

fn redraw(line: &str) {
    let mut err = std::io::stderr();
    let _ = write!(err, "\r\x1b[2K{line}");
    let _ = err.flush();
}

fn clear_line() {
    let mut err = std::io::stderr();
    let _ = write!(err, "\r\x1b[2K");
    let _ = err.flush();
}

/// A determinate progress bar for downloads. `total == 0` falls back to an
/// indeterminate byte counter.
pub struct Bar {
    label: String,
    total: u64,
    tty: bool,
    frame: usize,
    last_pct: i32,
}

impl Bar {
    pub fn new(label: impl Into<String>, total: u64) -> Bar {
        let label = label.into();
        let tty = stderr_is_terminal();
        if !tty {
            let suffix = if total > 0 {
                format!(" ({})", human_bytes(total))
            } else {
                String::new()
            };
            eprintln!("{label}{suffix} …");
        }
        Bar { label, total, tty, frame: 0, last_pct: -1 }
    }

    /// Update with the number of bytes received so far.
    pub fn set(&mut self, current: u64) {
        if !self.tty {
            return;
        }
        let pct = if self.total > 0 {
            ((current * 100) / self.total) as i32
        } else {
            -1
        };
        // Redraw on each percent step (or every call when total is unknown).
        if pct == self.last_pct {
            return;
        }
        self.last_pct = pct;
        self.frame = (self.frame + 1) % FRAMES.len();
        let spin = FRAMES[self.frame];

        if self.total > 0 {
            let filled = (current as usize * BAR_WIDTH / self.total as usize).min(BAR_WIDTH);
            let bar: String = "█".repeat(filled) + &"░".repeat(BAR_WIDTH - filled);
            redraw(&format!(
                "{spin} {} [{bar}] {pct:>3}%  {} / {}",
                self.label,
                human_bytes(current),
                human_bytes(self.total),
            ));
        } else {
            redraw(&format!("{spin} {} {}", self.label, human_bytes(current)));
        }
    }

    /// Finish with a checkmark summary line.
    pub fn finish(self, summary: &str) {
        if self.tty {
            clear_line();
        }
        eprintln!("✓ {summary}");
    }
}

/// An indeterminate spinner for steps without a known size. Without threads it
/// animates on each explicit `tick`; steps with no inner iterations simply show
/// a start frame and a finishing checkmark.
pub struct Spinner {
    label: String,
    tty: bool,
    frame: usize,
}

impl Spinner {
    pub fn new(label: impl Into<String>) -> Spinner {
        let label = label.into();
        let tty = stderr_is_terminal();
        if tty {
            redraw(&format!("{} {label} …", FRAMES[0]));
        } else {
            eprintln!("{label} …");
        }
        Spinner { label, tty, frame: 0 }
    }

    /// Advance the spinner, optionally updating the trailing detail.
    pub fn tick(&mut self, detail: &str) {
        if !self.tty {
            return;
        }
        self.frame = (self.frame + 1) % FRAMES.len();
        redraw(&format!("{} {} {detail}", FRAMES[self.frame], self.label));
    }

    pub fn finish(self, summary: &str) {
        if self.tty {
            clear_line();
        }
        eprintln!("✓ {summary}");
    }
}
