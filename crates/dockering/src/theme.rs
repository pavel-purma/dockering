//! Theme setup (SHL-009, SHL-024): System/Light/Dark from config, Docker-like accent
//! `#1D63ED`, live switching, and UI zoom via the theme base font size.

use dk_hub::ThemeMode;
use gpui_kit::component::{Theme, ThemeMode as KitMode};
use gpui_kit::{App, Hsla, Rgba, Window, px};

/// Docker blue.
pub const ACCENT: u32 = 0x1D63ED;
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
    Theme::update(cx, |theme| {
        theme.primary = accent;
        theme.primary_hover = accent.opacity(0.9);
        theme.primary_active = accent.opacity(0.8);
        theme.ring = if dark {
            accent.blend(Hsla::white().opacity(0.25))
        } else {
            accent
        };
        theme.link = accent;
        theme.link_hover = accent.opacity(0.85);
        theme.button_primary = accent;
        theme.button_primary_hover = accent.opacity(0.9);
        theme.button_primary_active = accent.opacity(0.8);
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
