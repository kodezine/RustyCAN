//! 80×30 character-cell console for the LCD boot terminal.
//!
//! The display is 640×480 pixels; at 8×16 glyphs that gives 80 columns × 30
//! rows (exactly fills the screen, no wasted pixels).
//!
//! The console maintains a logical grid of (character, fg, bg) triples and
//! delegates all pixel rendering to a [`Renderer`].  When the cursor reaches
//! the last row a single hardware-accelerated scroll (DMA2D bulk copy) moves
//! all rows up by one text row and the vacated bottom row is cleared.
//!
//! # Boot log format
//!
//! Each [`BootLogEntry`] is rendered as a full line:
//! ```text
//! [SSSSS.uuuuuu] description text              [  OK  ]
//! ```
//! The timestamp is in dim-green; the status tag uses colour coding.

use crate::renderer::{colors, Renderer};

/// Number of character columns (640 ÷ 8).
pub const COLS: u8 = 80;
/// Number of character rows (480 ÷ 16).
pub const ROWS: u8 = 30;

/// Status indicator for a boot-log line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, defmt::Format)]
pub enum BootStatus {
    /// Rendered as bright-green `[  OK  ]`
    Ok,
    /// Rendered as bright-red `[FAILED]`
    Failed,
    /// Rendered as bright-yellow `[ WARN ]`
    Warn,
    /// Rendered as cyan `[ INFO ]`
    Info,
    /// No status tag; line ends without a marker.
    None,
}

/// A single entry in the scrolling boot log.
#[derive(Clone, Copy, Debug)]
pub struct BootLogEntry {
    /// Timestamp in microseconds since firmware start.
    pub timestamp_us: u64,
    /// Human-readable message (truncated to fit the line).
    pub text: &'static str,
    /// Status to show at the right margin.
    pub status: BootStatus,
}

/// 80×30 character-cell console.
///
/// Stores only the colour of each cell so that `scroll_up` can re-colour
/// cleared cells correctly.  The actual glyph pixels live only in the SDRAM
/// framebuffer.
pub struct Console {
    /// Current cursor row (0-based).
    pub row: u8,
    /// Current cursor column (0-based).
    pub col: u8,
    /// First row reserved for log output; rows 0..top_row are the header and
    /// are never scrolled or overwritten by the console.
    pub top_row: u8,
    /// Per-cell foreground colour.
    fg: [[u16; COLS as usize]; ROWS as usize],
    /// Per-cell background colour.
    bg: [[u16; COLS as usize]; ROWS as usize],
}

impl Default for Console {
    fn default() -> Self {
        Self::new()
    }
}

impl Console {
    /// Create a new empty console (no screen output yet).
    pub const fn new() -> Self {
        Self {
            row: 0,
            col: 0,
            top_row: 0,
            fg: [[colors::FG_WHITE; COLS as usize]; ROWS as usize],
            bg: [[colors::BG_BLACK; COLS as usize]; ROWS as usize],
        }
    }

    // ── Internal helpers ──────────────────────────────────────────────────────

    /// Advance the cursor to the next line, scrolling if at the last row.
    fn newline_inner(&mut self, renderer: &Renderer) {
        self.col = 0;
        // Clamp: cursor must never enter the header area.
        if self.row < self.top_row {
            self.row = self.top_row;
            return;
        }
        if self.row < ROWS - 1 {
            self.row += 1;
        } else {
            // Scroll only the log region (top_row..ROWS) up by one text row.
            let top_y = self.top_row as u16 * 16;
            renderer.scroll_up_region(top_y, 1, colors::BG_BLACK);
            // Shift colour arrays for the log region only.
            let top = self.top_row as usize;
            for r in (top + 1)..ROWS as usize {
                self.fg[r - 1] = self.fg[r];
                self.bg[r - 1] = self.bg[r];
            }
            let last = (ROWS - 1) as usize;
            self.fg[last] = [colors::FG_WHITE; COLS as usize];
            self.bg[last] = [colors::BG_BLACK; COLS as usize];
            // row stays at ROWS-1
        }
    }

    // ── Public API ────────────────────────────────────────────────────────────

    /// Write a single byte to the screen at the current cursor position.
    pub fn put_char(&mut self, ch: u8, fg: u16, bg: u16, renderer: &Renderer) {
        if ch == b'\n' {
            self.newline_inner(renderer);
            return;
        }
        if self.col >= COLS {
            self.newline_inner(renderer);
        }
        renderer.draw_glyph(self.col, self.row, ch, fg, bg);
        self.fg[self.row as usize][self.col as usize] = fg;
        self.bg[self.row as usize][self.col as usize] = bg;
        self.col += 1;
    }

    /// Write an ASCII/CP437 string at the current cursor.
    pub fn write_str(&mut self, s: &str, fg: u16, bg: u16, renderer: &Renderer) {
        for b in s.bytes() {
            self.put_char(b, fg, bg, renderer);
        }
    }

    /// Write a string followed by a newline.
    pub fn writeln(&mut self, s: &str, fg: u16, bg: u16, renderer: &Renderer) {
        self.write_str(s, fg, bg, renderer);
        self.newline_inner(renderer);
    }

    /// Render a boot log entry in dmesg format.
    ///
    /// Format (80 chars wide):
    /// ```text
    /// [SSSSS.uuuuuu] message text...                               [  OK  ]
    /// ```
    /// - Timestamp (dim-green, 15 chars including brackets)
    /// - Message (up to ~58 chars, white on black)
    /// - Status tag at column 72 (8 chars wide) — colour-coded
    ///
    /// Lines are automatically wrapped to 80 columns.
    pub fn write_boot_log(&mut self, entry: &BootLogEntry, renderer: &Renderer) {
        // Start at column 0 of a fresh line
        if self.col != 0 {
            self.newline_inner(renderer);
        }

        // ── Timestamp  [SSSSS.uuuuuu] (15 chars) ────────────────────────────
        let secs = entry.timestamp_us / 1_000_000;
        let usecs = entry.timestamp_us % 1_000_000;

        self.put_char(b'[', colors::DIM_GREEN, colors::BG_BLACK, renderer);
        write_u64_padded(self, secs, 5, renderer, colors::DIM_GREEN);
        self.put_char(b'.', colors::DIM_GREEN, colors::BG_BLACK, renderer);
        write_u64_padded(self, usecs, 6, renderer, colors::DIM_GREEN);
        self.put_char(b']', colors::DIM_GREEN, colors::BG_BLACK, renderer);
        self.put_char(b' ', colors::FG_WHITE, colors::BG_BLACK, renderer);
        // Cursor now at column 16

        // ── Message text (cols 16–71, 56 chars) ──────────────────────────────
        let msg_end_col: u8 = 71;
        for b in entry.text.bytes().take((msg_end_col - 16) as usize) {
            self.put_char(b, colors::FG_WHITE, colors::BG_BLACK, renderer);
        }
        // Pad to column 71
        while self.col < msg_end_col {
            self.put_char(b' ', colors::FG_WHITE, colors::BG_BLACK, renderer);
        }

        // ── Status tag (cols 72–79, 8 chars) ─────────────────────────────────
        let (tag, tag_fg) = match entry.status {
            BootStatus::Ok => (b"[  OK  ]", colors::BRIGHT_GREEN),
            BootStatus::Failed => (b"[FAILED]", colors::BRIGHT_RED),
            BootStatus::Warn => (b"[ WARN ]", colors::BRIGHT_YELLOW),
            BootStatus::Info => (b"[ INFO ]", colors::CYAN),
            BootStatus::None => (b"        ", colors::FG_WHITE),
        };
        self.put_char(b' ', colors::FG_WHITE, colors::BG_BLACK, renderer);
        for &b in tag {
            self.put_char(b, tag_fg, colors::BG_BLACK, renderer);
        }

        // Advance to next line
        self.newline_inner(renderer);
    }

    /// Clear the entire screen and reset the cursor to (0, 0).
    pub fn clear(&mut self, renderer: &Renderer) {
        renderer.clear(colors::BG_BLACK);
        self.row = 0;
        self.col = 0;
        for r in 0..ROWS as usize {
            self.fg[r] = [colors::FG_WHITE; COLS as usize];
            self.bg[r] = [colors::BG_BLACK; COLS as usize];
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Write `value` as a zero-padded decimal with `width` digits.
fn write_u64_padded(console: &mut Console, value: u64, width: u8, renderer: &Renderer, fg: u16) {
    let mut digits: [u8; 20] = [b'0'; 20];
    let mut tmp = value;
    let mut len = 0usize;
    if tmp == 0 {
        digits[0] = b'0';
        len = 1;
    } else {
        while tmp > 0 {
            digits[len] = b'0' + (tmp % 10) as u8;
            tmp /= 10;
            len += 1;
        }
        digits[..len].reverse();
    }
    let pad = (width as usize).saturating_sub(len);
    for _ in 0..pad {
        console.put_char(b'0', fg, colors::BG_BLACK, renderer);
    }
    for &d in &digits[..len] {
        console.put_char(d, fg, colors::BG_BLACK, renderer);
    }
}
