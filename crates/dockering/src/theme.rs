//! Theme setup (SHL-009, SHL-024): System/Light/Dark from config, a violet accent
//! `#6E56CF`, live switching, and UI zoom via the theme base font size.

use dk_hub::ThemeMode;
use gpui_kit::component::{Theme, ThemeMode as KitMode};
use gpui_kit::{App, Hsla, Rgba, Window, px};

/// Brand accent (violet). Kept apart from the status hues (green/amber/red/sky, spec 30 §4).
pub const ACCENT: u32 = 0x6E56CF;
/// Base UI font size at 100 % zoom (spec 30 §4: 13 px UI font).
pub const BASE_FONT_SIZE: f32 = 14.0;
pub const MIN_SCALE: f32 = 0.7;
pub const MAX_SCALE: f32 = 1.6;
pub const SCALE_STEP: f32 = 0.1;

pub fn accent() -> Hsla {
    Hsla::from(Rgba {
        r: ((ACCENT >> 16) & 0xff) as f32 / 255.0,
        g: ((ACCENT >> 8) & 0xff) as f32 / 255.0,
        b: (ACCENT & 0xff) as f32 / 255.0,
        a: 1.0,
    })
}

/// The accent as a text/icon colour: lifted in dark mode so it reads on near-black
/// (≥ 4.5:1 in both themes).
pub fn accent_text(dark: bool) -> Hsla {
    if dark {
        accent().blend(Hsla::white().opacity(0.4))
    } else {
        accent()
    }
}

/// The concrete light/dark mode for a configured mode.
pub fn resolve(mode: ThemeMode, window: Option<&Window>, cx: &App) -> KitMode {
    match mode {
        ThemeMode::Light => KitMode::Light,
        ThemeMode::Dark => KitMode::Dark,
        ThemeMode::System => window
            .map(|w| w.appearance())
            .unwrap_or_else(|| cx.window_appearance())
            .into(),
    }
}

/// Applies `mode` and `scale`, then the accent overrides. Takes effect live in every window.
pub fn apply(mode: ThemeMode, scale: f32, window: Option<&mut Window>, cx: &mut App) {
    let kit = resolve(mode, window.as_deref(), cx);
    Theme::change(kit, None, cx);
    let accent = accent();
    let dark = kit.is_dark();
    let accent_text = accent_text(dark);
    Theme::update(cx, |theme| {
        theme.primary = accent;
        theme.primary_hover = accent.opacity(0.9);
        theme.primary_active = accent.opacity(0.8);
        theme.primary_foreground = Hsla::white();
        theme.ring = if dark {
            accent.blend(Hsla::white().opacity(0.25))
        } else {
            accent
        };
        theme.link = accent_text;
        theme.link_hover = accent_text.opacity(0.85);
        theme.button_primary = accent;
        theme.button_primary_hover = accent.opacity(0.9);
        theme.button_primary_active = accent.opacity(0.8);
        theme.button_primary_foreground = Hsla::white();
        // Row cursor and text selection take the accent instead of the kit's blue.
        theme.list_active = accent.opacity(0.14);
        theme.list_active_border = accent;
        theme.table_active = accent.opacity(0.14);
        theme.table_active_border = accent;
        theme.selection = accent.opacity(if dark { 0.45 } else { 0.25 });
        // Row hover: visible, but quieter than the cursor row.
        theme.table_hover = if dark {
            Hsla::white().opacity(0.045)
        } else {
            Hsla::black().opacity(0.035)
        };
        theme.font_size = px(BASE_FONT_SIZE * clamp_scale(scale));
        theme.focus_ring = true;
    });
}

pub fn clamp_scale(scale: f32) -> f32 {
    if scale.is_finite() {
        scale.clamp(MIN_SCALE, MAX_SCALE)
    } else {
        1.0
    }
}

/// Next zoom level, rounded to one decimal so repeated steps don't drift.
pub fn step_scale(scale: f32, delta: f32) -> f32 {
    let v = clamp_scale(scale + delta);
    (v * 10.0).round() / 10.0
}

/// The mode the theme toggle switches to (KBD-017): the opposite of what is shown now.
pub fn toggled(current_is_dark: bool) -> ThemeMode {
    if current_is_dark {
        ThemeMode::Light
    } else {
        ThemeMode::Dark
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_steps_are_clamped_and_rounded() {
        assert_eq!(step_scale(1.0, SCALE_STEP), 1.1);
        assert_eq!(step_scale(1.6, SCALE_STEP), 1.6);
        assert_eq!(step_scale(0.7, -SCALE_STEP), 0.7);
        assert_eq!(clamp_scale(f32::NAN), 1.0);
    }

    #[test]
    fn toggle_flips() {
        assert_eq!(toggled(true), ThemeMode::Light);
        assert_eq!(toggled(false), ThemeMode::Dark);
    }
}
