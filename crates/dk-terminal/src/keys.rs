//! Keystroke → PTY byte encoding (TRM-003, KBD-060…063, KBD-083).
//!
//! [`encode`] decides, for one GPUI [`Keystroke`], whether it:
//!
//! - is written to the PTY ([`KeyAction::Pty`]): control/navigation keys, Ctrl-chords, Alt-chords;
//! - belongs to the app ([`KeyAction::PassToApp`]): the reserved chords of KBD-061/062, and on
//!   macOS every `Cmd` chord;
//! - is ignored here ([`KeyAction::Ignore`]): plain printable text, which arrives through GPUI's
//!   `InputHandler` instead (IME, dead keys, AltGr, KBD-083), and keys without an encoding.
//!
//! Sequences follow xterm: cursor keys switch between CSI and SS3 with DECCKM, and modified
//! keys use the `CSI 1;<m> X` / `CSI n;<m> ~` parameter form with `m = 1 + shift + 2·alt + 4·ctrl`.

use gpui_kit::Keystroke;

/// Terminal modes that influence key encoding.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KeyModes {
    /// DECCKM (`CSI ? 1 h`): cursor keys send SS3 instead of CSI.
    pub app_cursor: bool,
    /// DECKPAM (`ESC =`). GPUI doesn't distinguish keypad keys, so this is informational.
    pub app_keypad: bool,
}

/// What to do with a keystroke.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyAction {
    /// Write these bytes to the PTY (and stop propagation).
    Pty(Vec<u8>),
    /// A reserved app chord: let it propagate to the app keymap.
    PassToApp,
    /// Not handled here (text arrives via the input handler, or the key has no encoding).
    Ignore,
}

/// Which platform conventions to apply (`Mod` = `Cmd` on macOS, `Ctrl` elsewhere).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyPlatform {
    MacOs,
    /// Windows and Linux.
    Other,
}

impl KeyPlatform {
    /// The platform this binary was built for.
    pub const CURRENT: KeyPlatform = if cfg!(target_os = "macos") {
        KeyPlatform::MacOs
    } else {
        KeyPlatform::Other
    };
}

/// Encode `ks` for the current platform.
pub fn encode(ks: &Keystroke, mode: KeyModes) -> KeyAction {
    encode_for(ks, mode, KeyPlatform::CURRENT)
}

/// Encode `ks` with explicit platform conventions (used by tests to cover every OS).
pub fn encode_for(ks: &Keystroke, mode: KeyModes, platform: KeyPlatform) -> KeyAction {
    if is_reserved(ks, platform) {
        return KeyAction::PassToApp;
    }
    let m = &ks.modifiers;
    // Shells don't use Cmd/Win/Super; the OS or the app owns those chords (KBD-062, KBD-084).
    if m.platform {
        return KeyAction::PassToApp;
    }
    let key = ks.key.as_str();
    let param = modifier_param(m.shift, m.alt, m.control);

    // Named keys first.
    if let Some(bytes) = named_key(key, param, mode, m.shift, m.alt, m.control) {
        return KeyAction::Pty(bytes);
    }
    if is_modifier_key(key) {
        return KeyAction::Ignore;
    }

    // Printable keys.
    let Some(base) = single_char(key) else {
        return KeyAction::Ignore;
    };
    let typed = ks
        .key_char
        .as_deref()
        .filter(|s| !s.is_empty() && !s.chars().any(char::is_control));

    // AltGr (reported as Ctrl+Alt on Windows) or macOS Option producing a different character:
    // that's text, it arrives through the input handler (KBD-083).
    if m.alt {
        let produces_text = typed.is_some_and(|t| t != key && !t.eq_ignore_ascii_case(key));
        if (m.control && produces_text && platform == KeyPlatform::Other)
            || (platform == KeyPlatform::MacOs && produces_text)
        {
            return KeyAction::Ignore;
        }
    }

    if m.control {
        let Some(c0) = ctrl_byte(base, typed) else {
            return KeyAction::Ignore;
        };
        let mut out = Vec::with_capacity(2);
        if m.alt {
            out.push(0x1b);
        }
        out.push(c0);
        return KeyAction::Pty(out);
    }

    if m.alt {
        let mut out = vec![0x1b];
        match typed {
            Some(t) => out.extend_from_slice(t.as_bytes()),
            None => {
                let c = if m.shift {
                    base.to_ascii_uppercase()
                } else {
                    base
                };
                let mut buf = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
        }
        return KeyAction::Pty(out);
    }

    // Plain (or shifted) printable text: handled by the input handler.
    KeyAction::Ignore
}

/// Reserved app chords (KBD-061, KBD-062, KBD-063, TRM-003).
fn is_reserved(ks: &Keystroke, platform: KeyPlatform) -> bool {
    let m = &ks.modifiers;
    let key = ks.key.as_str();
    let only = |ctrl: bool, alt: bool, shift: bool, cmd: bool| {
        m.control == ctrl && m.alt == alt && m.shift == shift && m.platform == cmd
    };
    let mac = platform == KeyPlatform::MacOs;
    // `Mod` + Shift.
    let mod_shift = if mac {
        only(false, false, true, true)
    } else {
        only(true, false, true, false)
    };

    match key {
        // Ctrl+Tab / Ctrl+Shift+Tab switch detail tabs on every OS (KBD-040/061).
        "tab" if only(true, false, false, false) || only(true, false, true, false) => true,
        // Escape hatch (KBD-061).
        "f6" if mod_shift => true,
        "escape" if mac && only(false, false, false, true) => true,
        // Sub-tabs (KBD-062).
        "t" | "w" if mod_shift => true,
        "pageup" | "pagedown" if only(true, false, false, false) => true,
        // Scrollback (KBD-063).
        "pageup" | "pagedown" if only(false, false, true, false) => true,
        // Copy / paste (TRM-003).
        "c" | "v" if !mac && only(true, false, true, false) => true,
        "c" | "v" if mac && only(false, false, false, true) => true,
        _ => mac && m.platform,
    }
}

fn modifier_param(shift: bool, alt: bool, ctrl: bool) -> u8 {
    1 + u8::from(shift) + 2 * u8::from(alt) + 4 * u8::from(ctrl)
}

fn is_modifier_key(key: &str) -> bool {
    matches!(
        key,
        "shift" | "control" | "alt" | "platform" | "function" | "capslock"
    )
}

fn single_char(key: &str) -> Option<char> {
    let mut chars = key.chars();
    let c = chars.next()?;
    chars.next().is_none().then_some(c)
}

/// Encode a named (non-printable) key, or `None` if `key` isn't one.
fn named_key(
    key: &str,
    param: u8,
    mode: KeyModes,
    shift: bool,
    alt: bool,
    ctrl: bool,
) -> Option<Vec<u8>> {
    let modified = param > 1;
    // Cursor-style keys: `CSI X` / `SS3 X`, or `CSI 1;m X` with modifiers.
    let cursor = |final_byte: u8, ss3: bool| -> Vec<u8> {
        if modified {
            format!("\x1b[1;{param}{}", final_byte as char).into_bytes()
        } else if ss3 {
            vec![0x1b, b'O', final_byte]
        } else {
            vec![0x1b, b'[', final_byte]
        }
    };
    // Tilde keys: `CSI n ~` or `CSI n;m ~`.
    let tilde = |n: u8| -> Vec<u8> {
        if modified {
            format!("\x1b[{n};{param}~").into_bytes()
        } else {
            format!("\x1b[{n}~").into_bytes()
        }
    };
    let esc_prefixed = |bytes: &[u8]| -> Vec<u8> {
        let mut out = Vec::with_capacity(bytes.len() + 1);
        if alt {
            out.push(0x1b);
        }
        out.extend_from_slice(bytes);
        out
    };

    let bytes = match key {
        "up" => cursor(b'A', mode.app_cursor),
        "down" => cursor(b'B', mode.app_cursor),
        "right" => cursor(b'C', mode.app_cursor),
        "left" => cursor(b'D', mode.app_cursor),
        "home" => cursor(b'H', mode.app_cursor),
        "end" => cursor(b'F', mode.app_cursor),
        "insert" => tilde(2),
        "delete" => tilde(3),
        "pageup" => tilde(5),
        "pagedown" => tilde(6),
        "f1" => cursor(b'P', true),
        "f2" => cursor(b'Q', true),
        "f3" => cursor(b'R', true),
        "f4" => cursor(b'S', true),
        "f5" => tilde(15),
        "f6" => tilde(17),
        "f7" => tilde(18),
        "f8" => tilde(19),
        "f9" => tilde(20),
        "f10" => tilde(21),
        "f11" => tilde(23),
        "f12" => tilde(24),
        "f13" => tilde(25),
        "f14" => tilde(26),
        "f15" => tilde(28),
        "f16" => tilde(29),
        "f17" => tilde(31),
        "f18" => tilde(32),
        "f19" => tilde(33),
        "f20" => tilde(34),
        "tab" if shift => b"\x1b[Z".to_vec(),
        "tab" => esc_prefixed(b"\t"),
        "enter" => esc_prefixed(b"\r"),
        "escape" => esc_prefixed(b"\x1b"),
        "backspace" if ctrl => esc_prefixed(b"\x08"),
        "backspace" => esc_prefixed(b"\x7f"),
        "space" if ctrl => esc_prefixed(b"\0"),
        "space" if alt => b"\x1b ".to_vec(),
        // Plain/shifted space is text (input handler).
        _ => return None,
    };
    Some(bytes)
}

/// The C0 byte for `Ctrl+<key>`, xterm-style.
fn ctrl_byte(base: char, typed: Option<&str>) -> Option<u8> {
    let lower = base.to_ascii_lowercase();
    if lower.is_ascii_lowercase() {
        return Some(lower as u8 & 0x1f);
    }
    // Punctuation: prefer the typed character (layout-aware), then the key itself.
    let candidates = typed
        .and_then(single_char)
        .into_iter()
        .chain(std::iter::once(base));
    for c in candidates {
        let byte = match c {
            '@' | '2' | ' ' => 0x00,
            '[' | '3' => 0x1b,
            '\\' | '4' => 0x1c,
            ']' | '5' => 0x1d,
            '^' | '6' | '~' => 0x1e,
            '_' | '7' | '/' | '-' => 0x1f,
            '8' | '?' => 0x7f,
            _ => continue,
        };
        return Some(byte);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ks(s: &str) -> Keystroke {
        Keystroke::parse(s).expect("valid keystroke")
    }

    /// A keystroke as a platform would report it, with the typed character.
    fn typed(s: &str, ch: &str) -> Keystroke {
        let mut k = ks(s);
        k.key_char = Some(ch.to_string());
        k
    }

    fn other(s: &str) -> KeyAction {
        encode_for(&ks(s), KeyModes::default(), KeyPlatform::Other)
    }

    fn mac(s: &str) -> KeyAction {
        encode_for(&ks(s), KeyModes::default(), KeyPlatform::MacOs)
    }

    fn pty(bytes: &[u8]) -> KeyAction {
        KeyAction::Pty(bytes.to_vec())
    }

    #[test]
    fn kbd_060_tab_and_esc_sent_to_pty() {
        for p in [KeyPlatform::Other, KeyPlatform::MacOs] {
            let enc = |s| encode_for(&ks(s), KeyModes::default(), p);
            assert_eq!(enc("tab"), pty(b"\t"));
            assert_eq!(enc("shift-tab"), pty(b"\x1b[Z"));
            assert_eq!(enc("escape"), pty(b"\x1b"));
            assert_eq!(enc("f6"), pty(b"\x1b[17~"));
            assert_eq!(enc("shift-f6"), pty(b"\x1b[17;2~"));
            assert_eq!(enc("enter"), pty(b"\r"));
            assert_eq!(enc("backspace"), pty(b"\x7f"));
            assert_eq!(enc("ctrl-backspace"), pty(b"\x08"));
        }
        // Alt+digit goes to the shell (readline), KBD-061.
        assert_eq!(other("alt-1"), pty(b"\x1b1"));
    }

    #[test]
    fn kbd_061_escape_hatch_not_sent_to_pty() {
        assert_eq!(other("ctrl-shift-f6"), KeyAction::PassToApp);
        assert_eq!(mac("cmd-shift-f6"), KeyAction::PassToApp);
        assert_eq!(mac("cmd-escape"), KeyAction::PassToApp);
        // Plain Esc on macOS still goes to the PTY.
        assert_eq!(mac("escape"), pty(b"\x1b"));
        // `ctrl-shift-f6` on macOS isn't the hatch: it's encoded.
        assert_eq!(mac("ctrl-shift-f6"), pty(b"\x1b[17;6~"));
    }

    #[test]
    fn kbd_062_reserved_chords_pass_to_app() {
        for s in [
            "ctrl-tab",
            "ctrl-shift-tab",
            "ctrl-shift-t",
            "ctrl-shift-w",
            "ctrl-pageup",
            "ctrl-pagedown",
            "shift-pageup",
            "shift-pagedown",
            "ctrl-shift-c",
            "ctrl-shift-v",
        ] {
            assert_eq!(other(s), KeyAction::PassToApp, "{s} on Windows/Linux");
        }
        for s in [
            "ctrl-tab",
            "ctrl-shift-tab",
            "cmd-shift-t",
            "cmd-shift-w",
            "ctrl-pageup",
            "ctrl-pagedown",
            "shift-pageup",
            "shift-pagedown",
            "cmd-c",
            "cmd-v",
            "cmd-k",
            "cmd-1",
            "cmd-shift-p",
        ] {
            assert_eq!(mac(s), KeyAction::PassToApp, "{s} on macOS");
        }
        // Not reserved: other Ctrl chords go to the shell.
        assert_eq!(other("ctrl-t"), pty(&[0x14]));
        assert_eq!(other("ctrl-w"), pty(&[0x17]));
        assert_eq!(other("ctrl-k"), pty(&[0x0b]));
        assert_eq!(other("ctrl-v"), pty(&[0x16]));
        assert_eq!(other("pageup"), pty(b"\x1b[5~"));
        // On macOS, Ctrl+Shift+C is not the copy chord: it's encoded as Ctrl+C.
        assert_eq!(mac("ctrl-shift-c"), pty(&[0x03]));
    }

    #[test]
    fn trm_003_ctrl_c_is_etx() {
        assert_eq!(other("ctrl-c"), pty(&[0x03]));
        assert_eq!(mac("ctrl-c"), pty(&[0x03]));
        assert_eq!(other("ctrl-d"), pty(&[0x04]));
        assert_eq!(other("ctrl-a"), pty(&[0x01]));
        assert_eq!(other("ctrl-z"), pty(&[0x1a]));
        assert_eq!(other("ctrl-space"), pty(&[0x00]));
        assert_eq!(other("ctrl-["), pty(&[0x1b]));
        assert_eq!(other("ctrl-\\"), pty(&[0x1c]));
        assert_eq!(other("ctrl-]"), pty(&[0x1d]));
        assert_eq!(other("ctrl-^"), pty(&[0x1e]));
        assert_eq!(other("ctrl-_"), pty(&[0x1f]));
        assert_eq!(other("ctrl-2"), pty(&[0x00]));
        // Plain printable characters arrive via the input handler.
        assert_eq!(other("a"), KeyAction::Ignore);
        assert_eq!(other("shift-a"), KeyAction::Ignore);
        assert_eq!(other("space"), KeyAction::Ignore);
        assert_eq!(other("shift"), KeyAction::Ignore);
        // Arrows and function keys are forwarded.
        assert_eq!(other("home"), pty(b"\x1b[H"));
        assert_eq!(other("end"), pty(b"\x1b[F"));
        assert_eq!(other("insert"), pty(b"\x1b[2~"));
        assert_eq!(other("delete"), pty(b"\x1b[3~"));
        assert_eq!(other("f1"), pty(b"\x1bOP"));
        assert_eq!(other("f4"), pty(b"\x1bOS"));
        assert_eq!(other("f5"), pty(b"\x1b[15~"));
        assert_eq!(other("f12"), pty(b"\x1b[24~"));
    }

    #[test]
    fn decckm_switches_arrow_encoding() {
        let normal = KeyModes::default();
        let app = KeyModes {
            app_cursor: true,
            ..Default::default()
        };
        let enc = |s, m| encode_for(&ks(s), m, KeyPlatform::Other);
        assert_eq!(enc("up", normal), pty(b"\x1b[A"));
        assert_eq!(enc("down", normal), pty(b"\x1b[B"));
        assert_eq!(enc("right", normal), pty(b"\x1b[C"));
        assert_eq!(enc("left", normal), pty(b"\x1b[D"));
        assert_eq!(enc("up", app), pty(b"\x1bOA"));
        assert_eq!(enc("left", app), pty(b"\x1bOD"));
        assert_eq!(enc("home", app), pty(b"\x1bOH"));
        assert_eq!(enc("end", app), pty(b"\x1bOF"));
        // Modified arrows always use CSI.
        assert_eq!(enc("ctrl-up", app), pty(b"\x1b[1;5A"));
    }

    #[test]
    fn modified_arrow_params() {
        assert_eq!(other("shift-up"), pty(b"\x1b[1;2A"));
        assert_eq!(other("alt-left"), pty(b"\x1b[1;3D"));
        assert_eq!(other("alt-shift-right"), pty(b"\x1b[1;4C"));
        assert_eq!(other("ctrl-right"), pty(b"\x1b[1;5C"));
        assert_eq!(other("ctrl-shift-left"), pty(b"\x1b[1;6D"));
        assert_eq!(other("ctrl-alt-down"), pty(b"\x1b[1;7B"));
        assert_eq!(other("ctrl-delete"), pty(b"\x1b[3;5~"));
        assert_eq!(other("alt-pageup"), pty(b"\x1b[5;3~"));
        assert_eq!(other("ctrl-f1"), pty(b"\x1b[1;5P"));
        assert_eq!(other("shift-f5"), pty(b"\x1b[15;2~"));
        assert_eq!(other("shift-home"), pty(b"\x1b[1;2H"));
    }

    #[test]
    fn alt_prefixes_esc() {
        assert_eq!(other("alt-b"), pty(b"\x1bb"));
        assert_eq!(other("alt-f"), pty(b"\x1bf"));
        assert_eq!(other("alt-shift-b"), pty(b"\x1bB"));
        assert_eq!(
            encode_for(
                &typed("alt-.", "."),
                KeyModes::default(),
                KeyPlatform::Other
            ),
            pty(b"\x1b.")
        );
        assert_eq!(other("alt-backspace"), pty(b"\x1b\x7f"));
        assert_eq!(other("alt-enter"), pty(b"\x1b\r"));
        assert_eq!(other("alt-escape"), pty(b"\x1b\x1b"));
        assert_eq!(other("ctrl-alt-a"), pty(b"\x1b\x01"));
        assert_eq!(mac("alt-b"), pty(b"\x1bb"));
    }

    #[test]
    fn kbd_083_altgr_and_option_text_is_not_encoded() {
        // Czech AltGr+V types '@' (reported as Ctrl+Alt+V with key_char '@').
        let altgr = typed("ctrl-alt-v", "@");
        assert_eq!(
            encode_for(&altgr, KeyModes::default(), KeyPlatform::Other),
            KeyAction::Ignore
        );
        // macOS Option+S types 'ß'.
        let option = typed("alt-s", "ß");
        assert_eq!(
            encode_for(&option, KeyModes::default(), KeyPlatform::MacOs),
            KeyAction::Ignore
        );
        // Win key chords never reach the shell.
        assert_eq!(other("cmd-e"), KeyAction::PassToApp);
    }
}
