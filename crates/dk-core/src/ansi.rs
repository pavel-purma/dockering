//! ANSI SGR parsing for logs (LOG-002). Basic SGR colours are kept; other escapes stripped.
//! SIGNATURES FIXED — bodies by `rust-core`.

use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AnsiColor {
    /// 0–15 basic/bright, 16–255 xterm palette.
    Indexed(u8),
    Rgb(u8, u8, u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct SgrStyle {
    pub fg: Option<AnsiColor>,
    pub bg: Option<AnsiColor>,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
}

impl SgrStyle {
    pub fn is_plain(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyledSpan {
    /// Byte range into `StyledLine::text`.
    pub range: Range<usize>,
    pub style: SgrStyle,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StyledLine {
    /// Text with all escapes removed, no trailing `\n` / `\r\n`.
    pub text: String,
    /// Non-plain spans only, sorted, non-overlapping.
    pub spans: Vec<StyledSpan>,
}

/// Stateful across chunks: an escape or UTF-8 sequence split across `push` calls is handled,
/// and SGR state carries over line breaks (like a terminal). Invalid UTF-8 → U+FFFD.
#[derive(Debug, Default)]
pub struct AnsiParser {
    _private: (),
}

impl AnsiParser {
    pub fn new() -> Self {
        Self::default()
    }
    /// Returns the complete lines contained in the input so far.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<StyledLine> {
        let _ = bytes;
        unimplemented!("rust-core")
    }
    /// Returns the pending partial line, if any.
    pub fn flush(&mut self) -> Option<StyledLine> {
        unimplemented!("rust-core")
    }
}

/// Remove all escape sequences.
pub fn strip_ansi(s: &str) -> String {
    let _ = s;
    unimplemented!("rust-core")
}

/// Standard xterm RGB for an indexed colour (0–255).
pub fn indexed_to_rgb(i: u8) -> (u8, u8, u8) {
    let _ = i;
    unimplemented!("rust-core")
}
