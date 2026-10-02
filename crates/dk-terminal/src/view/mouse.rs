//! xterm mouse-report encoding (TRM-002): X10/normal (`CSI M Cb Cx Cy`) and SGR 1006
//! (`CSI < b ; x ; y M|m`).

/// What happened to the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseReport {
    Press,
    Release,
    Motion,
}

/// Button codes as xterm numbers them.
pub mod button {
    pub const LEFT: u8 = 0;
    pub const MIDDLE: u8 = 1;
    pub const RIGHT: u8 = 2;
    /// Motion with no button held.
    pub const NONE: u8 = 3;
    pub const WHEEL_UP: u8 = 64;
    pub const WHEEL_DOWN: u8 = 65;
}

/// Modifier bits added to the button code.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReportModifiers {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

/// Encode one report. `col`/`row` are 0-based cells. Returns `None` when the position can't be
/// expressed in the legacy encoding (beyond column/row 223).
pub fn encode(
    kind: MouseReport,
    button: u8,
    col: usize,
    row: usize,
    mods: ReportModifiers,
    sgr: bool,
) -> Option<Vec<u8>> {
    let mut code = button;
    if mods.shift {
        code += 4;
    }
    if mods.alt {
        code += 8;
    }
    if mods.ctrl {
        code += 16;
    }
    if kind == MouseReport::Motion {
        code += 32;
    }

    if sgr {
        let suffix = if kind == MouseReport::Release {
            'm'
        } else {
            'M'
        };
        return Some(format!("\x1b[<{code};{};{}{suffix}", col + 1, row + 1).into_bytes());
    }

    // Legacy: release is reported as button 3 (wheel events have no release).
    if kind == MouseReport::Release {
        if button >= button::WHEEL_UP {
            return None;
        }
        code = (code & !0b11) | button::NONE;
    }
    let cb = 32u16 + u16::from(code);
    let cx = 32 + 1 + col;
    let cy = 32 + 1 + row;
    if cb > 255 || cx > 255 || cy > 255 {
        return None;
    }
    Some(vec![0x1b, b'[', b'M', cb as u8, cx as u8, cy as u8])
}

#[cfg(test)]
mod tests {
    use super::*;

    const NO_MODS: ReportModifiers = ReportModifiers {
        shift: false,
        alt: false,
        ctrl: false,
    };

    #[test]
    fn trm_002_sgr_mouse_reports() {
        let enc = |kind, button, col, row| encode(kind, button, col, row, NO_MODS, true);
        assert_eq!(
            enc(MouseReport::Press, button::LEFT, 0, 0),
            Some(b"\x1b[<0;1;1M".to_vec())
        );
        assert_eq!(
            enc(MouseReport::Release, button::LEFT, 9, 4),
            Some(b"\x1b[<0;10;5m".to_vec())
        );
        assert_eq!(
            enc(MouseReport::Motion, button::LEFT, 2, 3),
            Some(b"\x1b[<32;3;4M".to_vec())
        );
        assert_eq!(
            enc(MouseReport::Press, button::WHEEL_UP, 0, 0),
            Some(b"\x1b[<64;1;1M".to_vec())
        );
        let ctrl = ReportModifiers {
            ctrl: true,
            ..NO_MODS
        };
        assert_eq!(
            encode(MouseReport::Press, button::RIGHT, 0, 0, ctrl, true),
            Some(b"\x1b[<18;1;1M".to_vec())
        );
        // SGR has no coordinate limit.
        assert!(enc(MouseReport::Press, button::LEFT, 500, 300).is_some());
    }

    #[test]
    fn trm_002_legacy_mouse_reports() {
        let enc = |kind, button, col, row| encode(kind, button, col, row, NO_MODS, false);
        assert_eq!(
            enc(MouseReport::Press, button::LEFT, 0, 0),
            Some(vec![0x1b, b'[', b'M', 32, 33, 33])
        );
        assert_eq!(
            enc(MouseReport::Release, button::LEFT, 1, 2),
            Some(vec![0x1b, b'[', b'M', 35, 34, 35])
        );
        assert_eq!(
            enc(MouseReport::Press, button::WHEEL_DOWN, 0, 0),
            Some(vec![0x1b, b'[', b'M', 97, 33, 33])
        );
        assert_eq!(enc(MouseReport::Release, button::WHEEL_UP, 0, 0), None);
        assert_eq!(enc(MouseReport::Press, button::LEFT, 300, 0), None);
    }
}
