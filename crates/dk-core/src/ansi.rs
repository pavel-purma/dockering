//! ANSI SGR parsing for logs (LOG-002). Basic SGR colours are kept; other escapes stripped.
//!
//! The parser is a byte-level state machine, so escape sequences and UTF-8 sequences split
//! across `push` calls are handled, and the result doesn't depend on chunk boundaries.

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

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;
/// Longest CSI parameter string we keep; longer sequences are stripped but not applied.
const MAX_CSI_PARAMS: usize = 256;

// ───────────────────────────── escape state machine ─────────────────────────────

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum EscState {
    #[default]
    Ground,
    /// After ESC.
    Esc,
    /// ESC followed by intermediate bytes (0x20–0x2F), waiting for the final byte.
    EscIntermediate,
    /// CSI (`ESC [`), collecting parameters.
    Csi,
    /// OSC / DCS / SOS / PM / APC string, terminated by BEL or ST (`ESC \`).
    Str,
    /// ESC seen inside a string: `\` completes ST.
    StrEsc,
}

/// What the caller should do with the byte just fed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// The byte is ordinary input (text or a C0 control) in the ground state.
    Text,
    /// The byte was consumed by an escape sequence.
    Consumed,
    /// A complete SGR sequence ended; its parameters are in `EscMachine::params`.
    Sgr,
    /// The byte aborted a sequence; feed it again (the machine is now in the ground state).
    Reprocess,
}

#[derive(Debug, Default)]
struct EscMachine {
    state: EscState,
    params: Vec<u8>,
    /// Intermediate bytes, a private marker or an overlong parameter string were seen:
    /// the sequence is not a plain SGR.
    not_sgr: bool,
}

impl EscMachine {
    fn step(&mut self, b: u8) -> Step {
        match self.state {
            EscState::Ground => {
                if b == ESC {
                    self.state = EscState::Esc;
                    Step::Consumed
                } else {
                    Step::Text
                }
            }
            EscState::Esc => match b {
                b'[' => {
                    self.state = EscState::Csi;
                    self.params.clear();
                    self.not_sgr = false;
                    Step::Consumed
                }
                // OSC, DCS, SOS, PM, APC.
                b']' | b'P' | b'X' | b'^' | b'_' => {
                    self.state = EscState::Str;
                    Step::Consumed
                }
                ESC => Step::Consumed,
                0x20..=0x2f => {
                    self.state = EscState::EscIntermediate;
                    Step::Consumed
                }
                0x30..=0x7e => {
                    self.state = EscState::Ground;
                    Step::Consumed
                }
                0x7f => Step::Consumed,
                _ => self.abort(),
            },
            EscState::EscIntermediate => match b {
                0x20..=0x2f | 0x7f => Step::Consumed,
                0x30..=0x7e => {
                    self.state = EscState::Ground;
                    Step::Consumed
                }
                _ => self.abort(),
            },
            EscState::Csi => match b {
                0x30..=0x3f => {
                    if self.params.is_empty() && (b'<'..=b'?').contains(&b) {
                        self.not_sgr = true;
                    }
                    if self.params.len() < MAX_CSI_PARAMS {
                        self.params.push(b);
                    } else {
                        self.not_sgr = true;
                    }
                    Step::Consumed
                }
                0x20..=0x2f => {
                    self.not_sgr = true;
                    Step::Consumed
                }
                0x40..=0x7e => {
                    self.state = EscState::Ground;
                    if b == b'm' && !self.not_sgr {
                        Step::Sgr
                    } else {
                        Step::Consumed
                    }
                }
                0x7f => Step::Consumed,
                _ => self.abort(),
            },
            EscState::Str => match b {
                BEL => {
                    self.state = EscState::Ground;
                    Step::Consumed
                }
                ESC => {
                    self.state = EscState::StrEsc;
                    Step::Consumed
                }
                // An unterminated string must not swallow the rest of the log.
                b'\n' => self.abort(),
                _ => Step::Consumed,
            },
            EscState::StrEsc => {
                if b == b'\\' {
                    self.state = EscState::Ground;
                    Step::Consumed
                } else {
                    // The ESC ended the string and starts a new sequence.
                    self.state = EscState::Esc;
                    Step::Reprocess
                }
            }
        }
    }

    fn abort(&mut self) -> Step {
        self.state = EscState::Ground;
        Step::Reprocess
    }

    fn reset(&mut self) {
        self.state = EscState::Ground;
        self.params.clear();
        self.not_sgr = false;
    }
}

// ───────────────────────────── incremental UTF-8 ─────────────────────────────

#[derive(Debug, Default)]
struct Utf8Decoder {
    buf: [u8; 4],
    len: usize,
    need: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Utf8Step {
    Pending,
    Char(char),
    /// Emit U+FFFD; the byte was consumed.
    Invalid,
    /// Emit U+FFFD for the pending prefix, then feed the byte again.
    InvalidReprocess,
}

impl Utf8Decoder {
    fn is_pending(&self) -> bool {
        self.len > 0
    }

    fn feed(&mut self, b: u8) -> Utf8Step {
        if self.len == 0 {
            self.need = match b {
                0x00..=0x7f => return Utf8Step::Char(b as char),
                0xc2..=0xdf => 2,
                0xe0..=0xef => 3,
                0xf0..=0xf4 => 4,
                _ => return Utf8Step::Invalid,
            };
            self.buf[0] = b;
            self.len = 1;
            return Utf8Step::Pending;
        }
        let range = if self.len == 1 {
            match self.buf[0] {
                0xe0 => 0xa0..=0xbf,
                0xed => 0x80..=0x9f,
                0xf0 => 0x90..=0xbf,
                0xf4 => 0x80..=0x8f,
                _ => 0x80..=0xbf,
            }
        } else {
            0x80..=0xbf
        };
        if !range.contains(&b) {
            self.len = 0;
            return Utf8Step::InvalidReprocess;
        }
        self.buf[self.len] = b;
        self.len += 1;
        if self.len < self.need {
            return Utf8Step::Pending;
        }
        let c = std::str::from_utf8(&self.buf[..self.len])
            .ok()
            .and_then(|s| s.chars().next())
            .unwrap_or(char::REPLACEMENT_CHARACTER);
        self.len = 0;
        Utf8Step::Char(c)
    }

    /// Drops an incomplete sequence; returns whether one was pending.
    fn take_pending(&mut self) -> bool {
        std::mem::replace(&mut self.len, 0) > 0
    }
}

// ───────────────────────────── parser ─────────────────────────────

/// Stateful across chunks: an escape or UTF-8 sequence split across `push` calls is handled,
/// and SGR state carries over line breaks (like a terminal). Invalid UTF-8 → U+FFFD.
///
/// Line endings: `\n` and `\r\n` end a line. A lone `\r` keeps only the text after it
/// (progress bars); a trailing `\r` with nothing after it keeps the line as it was.
#[derive(Debug, Default)]
pub struct AnsiParser {
    esc: EscMachine,
    utf8: Utf8Decoder,
    style: SgrStyle,
    line: StyledLine,
    /// A `\r` was seen; the next printed character clears the line first.
    pending_cr: bool,
}

impl AnsiParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the complete lines contained in the input so far.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<StyledLine> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            let b = bytes[i];
            if self.utf8.is_pending() {
                match self.utf8.feed(b) {
                    Utf8Step::Pending => {}
                    Utf8Step::Char(c) => self.put_char(c),
                    Utf8Step::Invalid => self.put_char(char::REPLACEMENT_CHARACTER),
                    Utf8Step::InvalidReprocess => {
                        self.put_char(char::REPLACEMENT_CHARACTER);
                        continue;
                    }
                }
                i += 1;
                continue;
            }
            match self.esc.step(b) {
                Step::Text => self.ground_byte(b, &mut out),
                Step::Consumed => {}
                Step::Sgr => apply_sgr(&mut self.style, &self.esc.params),
                Step::Reprocess => continue,
            }
            i += 1;
        }
        out
    }

    /// Returns the pending partial line, if any.
    pub fn flush(&mut self) -> Option<StyledLine> {
        if self.utf8.take_pending() {
            self.put_char(char::REPLACEMENT_CHARACTER);
        }
        self.esc.reset();
        self.pending_cr = false;
        let line = std::mem::take(&mut self.line);
        (!line.text.is_empty()).then_some(line)
    }

    fn ground_byte(&mut self, b: u8, out: &mut Vec<StyledLine>) {
        match b {
            b'\n' => {
                self.pending_cr = false;
                out.push(std::mem::take(&mut self.line));
            }
            b'\r' => self.pending_cr = true,
            b'\t' => self.put_char('\t'),
            0x00..=0x1f | 0x7f => {}
            0x20..=0x7e => self.put_char(b as char),
            _ => match self.utf8.feed(b) {
                Utf8Step::Char(c) => self.put_char(c),
                Utf8Step::Pending => {}
                Utf8Step::Invalid | Utf8Step::InvalidReprocess => {
                    self.put_char(char::REPLACEMENT_CHARACTER)
                }
            },
        }
    }

    fn put_char(&mut self, c: char) {
        // C1 controls (U+0080–U+009F) are invisible; drop them like C0.
        if ('\u{80}'..='\u{9f}').contains(&c) {
            return;
        }
        if std::mem::take(&mut self.pending_cr) {
            self.line.text.clear();
            self.line.spans.clear();
        }
        let start = self.line.text.len();
        self.line.text.push(c);
        let end = self.line.text.len();
        if self.style.is_plain() {
            return;
        }
        match self.line.spans.last_mut() {
            Some(last) if last.style == self.style && last.range.end == start => {
                last.range.end = end;
            }
            _ => self.line.spans.push(StyledSpan {
                range: start..end,
                style: self.style,
            }),
        }
    }
}

/// Parses SGR parameters (`1;38;5;208`, `38:2::r:g:b`, …) into `style`.
fn apply_sgr(style: &mut SgrStyle, params: &[u8]) {
    // Each `;`-separated group may carry `:` sub-parameters. Empty values are `None`.
    let groups: Vec<Vec<Option<u32>>> = params
        .split(|&b| b == b';')
        .map(|g| g.split(|&b| b == b':').map(parse_num).collect())
        .collect();
    let mut i = 0;
    while i < groups.len() {
        let g = &groups[i];
        let code = g.first().copied().flatten().unwrap_or(0);
        i += 1;
        match code {
            0 => *style = SgrStyle::default(),
            1 => style.bold = true,
            2 => style.dim = true,
            3 => style.italic = true,
            // `4:0` turns underline off; other `4:n` styles are still an underline.
            4 => style.underline = g.get(1).copied().flatten() != Some(0),
            7 => style.inverse = true,
            22 => {
                style.bold = false;
                style.dim = false;
            }
            23 => style.italic = false,
            24 => style.underline = false,
            27 => style.inverse = false,
            30..=37 => style.fg = Some(AnsiColor::Indexed((code - 30) as u8)),
            39 => style.fg = None,
            40..=47 => style.bg = Some(AnsiColor::Indexed((code - 40) as u8)),
            49 => style.bg = None,
            90..=97 => style.fg = Some(AnsiColor::Indexed((code - 90 + 8) as u8)),
            100..=107 => style.bg = Some(AnsiColor::Indexed((code - 100 + 8) as u8)),
            38 | 48 | 58 => {
                let color = if g.len() > 1 {
                    extended_color_colon(&g[1..])
                } else {
                    let (color, used) = extended_color_semicolon(&groups[i..]);
                    i += used;
                    color
                };
                match (code, color) {
                    (38, Some(c)) => style.fg = Some(c),
                    (48, Some(c)) => style.bg = Some(c),
                    // 58 (underline colour) is consumed but not rendered.
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

fn parse_num(digits: &[u8]) -> Option<u32> {
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    Some(digits.iter().fold(0u32, |acc, &d| {
        acc.saturating_mul(10).saturating_add(u32::from(d - b'0'))
    }))
}

fn to_u8(v: Option<u32>) -> Option<u8> {
    u8::try_from(v.unwrap_or(0)).ok()
}

/// `38:5:n`, `38:2:r:g:b` or `38:2:<colorspace>:r:g:b` (sub-parameters after the code).
fn extended_color_colon(sub: &[Option<u32>]) -> Option<AnsiColor> {
    match sub.first().copied().flatten()? {
        5 => Some(AnsiColor::Indexed(to_u8(*sub.get(1)?)?)),
        2 => {
            let rgb = if sub.len() >= 5 {
                &sub[2..5]
            } else {
                sub.get(1..4)?
            };
            Some(AnsiColor::Rgb(
                to_u8(rgb[0])?,
                to_u8(rgb[1])?,
                to_u8(rgb[2])?,
            ))
        }
        _ => None,
    }
}

/// `38;5;n` or `38;2;r;g;b`: returns the colour and how many groups after the code it used.
fn extended_color_semicolon(rest: &[Vec<Option<u32>>]) -> (Option<AnsiColor>, usize) {
    let value = |k: usize| rest.get(k).and_then(|g| g.first().copied().flatten());
    match value(0) {
        Some(5) if rest.len() >= 2 => (to_u8(value(1)).map(AnsiColor::Indexed), 2),
        Some(2) if rest.len() >= 4 => {
            let rgb = (to_u8(value(1)), to_u8(value(2)), to_u8(value(3)));
            let color = match rgb {
                (Some(r), Some(g), Some(b)) => Some(AnsiColor::Rgb(r, g, b)),
                _ => None,
            };
            (color, 4)
        }
        // Malformed: ignore the rest of the sequence, like terminals do.
        _ => (None, rest.len()),
    }
}

/// Remove all escape sequences. Other text, including `\n`, `\r` and `\t`, is kept; other
/// C0 controls and DEL are dropped.
pub fn strip_ansi(s: &str) -> String {
    let mut esc = EscMachine::default();
    let mut out = Vec::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        match esc.step(b) {
            Step::Text => {
                if !matches!(b, 0x00..=0x08 | 0x0b | 0x0c | 0x0e..=0x1f | 0x7f) {
                    out.push(b);
                }
            }
            Step::Consumed | Step::Sgr => {}
            Step::Reprocess => continue,
        }
        i += 1;
    }
    // Whole characters are either kept or swallowed, so this is valid UTF-8; stay lossless
    // anyway rather than panic.
    match String::from_utf8(out) {
        Ok(s) => s,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    }
}

/// Standard xterm RGB for an indexed colour (0–255).
pub fn indexed_to_rgb(i: u8) -> (u8, u8, u8) {
    const BASIC: [(u8, u8, u8); 16] = [
        (0, 0, 0),
        (205, 0, 0),
        (0, 205, 0),
        (205, 205, 0),
        (0, 0, 238),
        (205, 0, 205),
        (0, 205, 205),
        (229, 229, 229),
        (127, 127, 127),
        (255, 0, 0),
        (0, 255, 0),
        (255, 255, 0),
        (92, 92, 255),
        (255, 0, 255),
        (0, 255, 255),
        (255, 255, 255),
    ];
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    match i {
        0..=15 => BASIC[i as usize],
        16..=231 => {
            let n = i - 16;
            (
                LEVELS[(n / 36) as usize],
                LEVELS[((n / 6) % 6) as usize],
                LEVELS[(n % 6) as usize],
            )
        }
        232..=255 => {
            let v = 8 + 10 * (i - 232);
            (v, v, v)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn parse_all(bytes: &[u8]) -> Vec<StyledLine> {
        let mut p = AnsiParser::new();
        let mut lines = p.push(bytes);
        lines.extend(p.flush());
        lines
    }

    fn fg(c: AnsiColor) -> SgrStyle {
        SgrStyle {
            fg: Some(c),
            ..Default::default()
        }
    }

    fn span(range: Range<usize>, style: SgrStyle) -> StyledSpan {
        StyledSpan { range, style }
    }

    #[test]
    fn log_002_colors_kept() {
        let lines =
            parse_all(b"\x1b[31mred\x1b[0m plain \x1b[1;38;5;208mx\x1b[38;2;1;2;3my\x1b[m\n");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "red plain xy");
        let bold = |c| SgrStyle {
            bold: true,
            ..fg(c)
        };
        assert_eq!(
            lines[0].spans,
            [
                span(0..3, fg(AnsiColor::Indexed(1))),
                span(10..11, bold(AnsiColor::Indexed(208))),
                span(11..12, bold(AnsiColor::Rgb(1, 2, 3))),
            ]
        );

        // Bright, background, attribute toggles, colon variants, and span merging.
        let line = &parse_all(
            b"\x1b[91;104ma\x1b[91mb\x1b[39;49;2;3;4;7mc\x1b[22;23;24;27md\
              \x1b[38:5:9me\x1b[48:2::10:20:30mf\x1b[38:2:7:8:9;4:0mg\x1b[0m",
        )[0];
        assert_eq!(line.text, "abcdefg");
        let dim_all = SgrStyle {
            dim: true,
            italic: true,
            underline: true,
            inverse: true,
            ..Default::default()
        };
        assert_eq!(
            line.spans,
            [
                span(
                    0..2,
                    SgrStyle {
                        bg: Some(AnsiColor::Indexed(12)),
                        ..fg(AnsiColor::Indexed(9))
                    }
                ),
                span(2..3, dim_all),
                span(4..5, fg(AnsiColor::Indexed(9))),
                span(
                    5..6,
                    SgrStyle {
                        bg: Some(AnsiColor::Rgb(10, 20, 30)),
                        ..fg(AnsiColor::Indexed(9))
                    }
                ),
                span(
                    6..7,
                    SgrStyle {
                        bg: Some(AnsiColor::Rgb(10, 20, 30)),
                        ..fg(AnsiColor::Rgb(7, 8, 9))
                    }
                ),
            ]
        );
    }

    #[test]
    fn log_002_other_escapes_stripped() {
        let lines = parse_all(
            b"\x1b[2K\x1b[1;1H\x1b]0;title\x07a\x1b]8;;http://x\x1b\\b\x1b(Bc\x1b7d\x07e\x08\tf\
              \x1b[?25l\x1b[>4;1mg\x1bPdcs\x1b\\h\x00\x7fi\n",
        );
        assert_eq!(
            lines,
            [StyledLine {
                text: "abcde\tfghi".into(),
                spans: vec![],
            }]
        );
        // An unterminated OSC doesn't swallow the following lines.
        let lines = parse_all(b"x\x1b]0;never ends\nnext\n");
        let texts: Vec<_> = lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, ["x", "next"]);
        // strip_ansi keeps line structure but drops escapes and other controls.
        assert_eq!(
            strip_ansi("\x1b[1;31merror\x1b[0m: \x1b]0;t\x07bad\x07\r\n\tok\x1b"),
            "error: bad\r\n\tok"
        );
    }

    #[test]
    fn log_002_split_chunks() {
        let input =
            "\x1b[1;38;2;10;20;30mhé🦀llo\x1b[0m \x1b]0;title\x1b\\wörld\r\nnext\n".as_bytes();
        let whole = parse_all(input);
        assert_eq!(whole[0].text, "hé🦀llo wörld");
        assert_eq!(whole[1].text, "next");
        // Byte by byte.
        let mut p = AnsiParser::new();
        let mut lines = Vec::new();
        for b in input {
            lines.extend(p.push(std::slice::from_ref(b)));
        }
        lines.extend(p.flush());
        assert_eq!(lines, whole);
        // Every two-way split.
        for k in 0..=input.len() {
            let mut p = AnsiParser::new();
            let mut lines = p.push(&input[..k]);
            lines.extend(p.push(&input[k..]));
            lines.extend(p.flush());
            assert_eq!(lines, whole, "split at {k}");
        }
    }

    #[test]
    fn log_002_sgr_carries_across_lines() {
        let mut p = AnsiParser::new();
        let lines = p.push(b"\x1b[32mone\ntwo\x1b[0m\nthree\n");
        let green = fg(AnsiColor::Indexed(2));
        assert_eq!(lines[0].spans, [span(0..3, green)]);
        assert_eq!(lines[1].spans, [span(0..3, green)]);
        assert!(lines[2].spans.is_empty());
        // And across pushes.
        let mut p = AnsiParser::new();
        p.push(b"\x1b[33m");
        let lines = p.push(b"warn\n");
        assert_eq!(lines[0].spans, [span(0..4, fg(AnsiColor::Indexed(3)))]);
    }

    #[test]
    fn log_002_carriage_return() {
        let texts = |bytes: &[u8]| -> Vec<String> {
            parse_all(bytes).into_iter().map(|l| l.text).collect()
        };
        assert_eq!(texts(b"progress 10%\rprogress 100%\n"), ["progress 100%"]);
        assert_eq!(texts(b"abc\r\ndef\r\n"), ["abc", "def"]);
        assert_eq!(texts(b"a\rb\rc\n"), ["c"]);
        assert_eq!(texts(b"keep me\r"), ["keep me"]);
        // Spans before the `\r` are dropped too.
        let line = &parse_all(b"\x1b[31mold\rnew\n")[0];
        assert_eq!(line.text, "new");
        assert_eq!(line.spans, [span(0..3, fg(AnsiColor::Indexed(1)))]);
        // `\r` at a chunk boundary.
        let mut p = AnsiParser::new();
        assert!(p.push(b"abc\r").is_empty());
        assert_eq!(p.push(b"\ndef\n").len(), 2);
        let mut p = AnsiParser::new();
        p.push(b"abc\r");
        assert_eq!(p.push(b"x\n")[0].text, "x");
    }

    #[test]
    fn log_002_invalid_utf8() {
        let lines = parse_all(b"a\xffb\xe2\x82c\xed\xa0\x80d\xc0\xafe\n");
        assert_eq!(
            lines[0].text,
            "a\u{FFFD}b\u{FFFD}c\u{FFFD}\u{FFFD}\u{FFFD}d\u{FFFD}\u{FFFD}e"
        );
        // A valid sequence split across pushes is decoded.
        let mut p = AnsiParser::new();
        p.push(b"\xe2\x82");
        assert_eq!(p.push(b"\xac\n")[0].text, "€");
        // An incomplete sequence at the end becomes U+FFFD on flush.
        let mut p = AnsiParser::new();
        p.push(b"x\xe2\x82");
        assert_eq!(p.flush().unwrap().text, "x\u{FFFD}");
        // ESC interrupting a sequence.
        assert_eq!(parse_all(b"\xe2\x1b[31mz")[0].text, "\u{FFFD}z");
    }

    #[test]
    fn log_002_flush_returns_partial_line() {
        let mut p = AnsiParser::new();
        assert!(p.push(b"partial").is_empty());
        assert_eq!(p.flush().unwrap().text, "partial");
        assert_eq!(p.flush(), None);
        p.push(b"\x1b[31m");
        assert_eq!(p.flush(), None);
    }

    #[test]
    fn log_002_indexed_palette() {
        assert_eq!(indexed_to_rgb(0), (0, 0, 0));
        assert_eq!(indexed_to_rgb(1), (205, 0, 0));
        assert_eq!(indexed_to_rgb(12), (92, 92, 255));
        assert_eq!(indexed_to_rgb(15), (255, 255, 255));
        assert_eq!(indexed_to_rgb(16), (0, 0, 0));
        assert_eq!(indexed_to_rgb(196), (255, 0, 0));
        assert_eq!(indexed_to_rgb(208), (255, 135, 0));
        assert_eq!(indexed_to_rgb(231), (255, 255, 255));
        assert_eq!(indexed_to_rgb(232), (8, 8, 8));
        assert_eq!(indexed_to_rgb(255), (238, 238, 238));
    }

    fn check_line(line: &StyledLine) -> Result<(), TestCaseError> {
        prop_assert!(!line.text.contains('\x1b'));
        prop_assert!(!line.text.contains('\n'));
        let mut prev_end = 0;
        for s in &line.spans {
            prop_assert!(!s.style.is_plain());
            prop_assert!(s.range.start >= prev_end && s.range.start < s.range.end);
            prop_assert!(s.range.end <= line.text.len());
            prop_assert!(line.text.is_char_boundary(s.range.start));
            prop_assert!(line.text.is_char_boundary(s.range.end));
            prev_end = s.range.end;
        }
        Ok(())
    }

    fn interesting_bytes() -> impl Strategy<Value = Vec<u8>> {
        // Bias towards escape-relevant bytes so sequences actually get exercised.
        let byte = prop_oneof![
            any::<u8>(),
            prop::sample::select(b"\x1b[];:m0123456789\r\n\x07\\Pa\xe2\x82\xac".to_vec()),
        ];
        prop::collection::vec(byte, 0..512)
    }

    proptest! {
        #[test]
        fn log_002_arbitrary_bytes_never_panic(bytes in interesting_bytes(), split in any::<prop::sample::Index>()) {
            let whole = parse_all(&bytes);
            for line in &whole {
                check_line(line)?;
            }
            let k = split.index(bytes.len() + 1);
            let mut p = AnsiParser::new();
            let mut lines = p.push(&bytes[..k]);
            lines.extend(p.push(&bytes[k..]));
            lines.extend(p.flush());
            prop_assert_eq!(lines, whole);
        }

        #[test]
        fn log_002_strip_ansi_never_contains_esc(s in any::<String>(), bytes in interesting_bytes()) {
            prop_assert!(!strip_ansi(&s).contains('\x1b'));
            let lossy = String::from_utf8_lossy(&bytes);
            prop_assert!(!strip_ansi(&lossy).contains('\x1b'));
        }
    }
}
