//! Terminal colours derived from the GPUI Kit theme (TRM-002).
//!
//! Background, foreground, cursor and selection come from the active theme; the 16 ANSI colours
//! use a standard xterm-style palette with light and dark variants; indices 16–255 are the
//! xterm 6×6×6 cube and grey ramp. Colours set by the application (OSC 4/10/11) win.

use std::hash::{Hash, Hasher};

use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};
use gpui_kit::component::ActiveTheme;
use gpui_kit::{App, Hsla, Rgba, rgb};

const DARK_ANSI: [u32; 16] = [
    0x000000, 0xcd3131, 0x0dbc79, 0xe5e510, 0x2472c8, 0xbc3fbc, 0x11a8cd, 0xe5e5e5, //
    0x666666, 0xf14c4c, 0x23d18b, 0xf5f543, 0x3b8eea, 0xd670d6, 0x29b8db, 0xffffff,
];

const LIGHT_ANSI: [u32; 16] = [
    0x000000, 0xcd3131, 0x00a000, 0x949800, 0x0451a5, 0xbc05bc, 0x0598bc, 0x555555, //
    0x666666, 0xcd3131, 0x14ce14, 0xb5ba00, 0x0451a5, 0xbc05bc, 0x0598bc, 0xa5a5a5,
];

/// Resolved colours for one frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Palette {
    pub background: Hsla,
    pub foreground: Hsla,
    pub cursor: Hsla,
    pub selection: Hsla,
    pub dark: bool,
    ansi: [Hsla; 16],
}

impl Palette {
    pub fn new(
        background: Hsla,
        foreground: Hsla,
        cursor: Hsla,
        selection: Hsla,
        dark: bool,
    ) -> Self {
        let table = if dark { &DARK_ANSI } else { &LIGHT_ANSI };
        Self {
            background,
            foreground,
            cursor,
            selection,
            dark,
            ansi: table.map(|hex| rgb(hex).into()),
        }
    }

    /// The palette for the active GPUI Kit theme.
    pub fn from_theme(cx: &App) -> Self {
        let theme = cx.theme();
        Self::new(
            theme.background,
            theme.foreground,
            theme.caret,
            theme.selection,
            theme.is_dark(),
        )
    }

    /// A stable key of everything that affects text colour (for row-cache invalidation).
    pub fn key(&self, overrides: &Colors) -> u64 {
        let mut h = std::hash::DefaultHasher::new();
        for c in [self.background, self.foreground] {
            hash_hsla(c, &mut h);
        }
        self.dark.hash(&mut h);
        for i in 0..alacritty_terminal::term::color::COUNT {
            if let Some(rgb) = overrides[i] {
                (i, rgb.r, rgb.g, rgb.b).hash(&mut h);
            }
        }
        h.finish()
    }

    /// Colour for a cell attribute, honouring application overrides.
    pub fn resolve(&self, color: Color, overrides: &Colors) -> Hsla {
        match color {
            Color::Spec(rgb) => from_rgb(rgb),
            Color::Indexed(i) => match overrides[usize::from(i)] {
                Some(rgb) => from_rgb(rgb),
                None => self.indexed(i),
            },
            Color::Named(name) => match overrides[name as usize] {
                Some(rgb) => from_rgb(rgb),
                None => self.named(name),
            },
        }
    }

    /// The RGB value reported for an OSC colour query on `index`.
    pub fn query(&self, index: usize, overrides: &Colors) -> Rgb {
        if let Some(rgb) = overrides.get(index) {
            return rgb;
        }
        let color = match index {
            0..=255 => self.indexed(index as u8),
            256 => self.foreground,
            257 => self.background,
            258 => self.cursor,
            _ => self.foreground,
        };
        to_rgb(color)
    }

    pub fn indexed(&self, i: u8) -> Hsla {
        match i {
            0..=15 => self.ansi[usize::from(i)],
            16..=231 => {
                let i = i - 16;
                let level = |v: u8| if v == 0 { 0 } else { 55 + 40 * v };
                from_rgb(Rgb {
                    r: level(i / 36),
                    g: level((i / 6) % 6),
                    b: level(i % 6),
                })
            }
            232..=255 => {
                let v = 8 + 10 * (i - 232);
                from_rgb(Rgb { r: v, g: v, b: v })
            }
        }
    }

    fn named(&self, name: NamedColor) -> Hsla {
        use NamedColor as N;
        match name {
            N::Foreground | N::BrightForeground => self.foreground,
            N::Background => self.background,
            N::Cursor => self.cursor,
            N::DimForeground => self.dim(self.foreground),
            N::DimBlack
            | N::DimRed
            | N::DimGreen
            | N::DimYellow
            | N::DimBlue
            | N::DimMagenta
            | N::DimCyan
            | N::DimWhite => self.dim(self.ansi[name.to_bright() as usize]),
            other => {
                let index = other as usize;
                self.ansi.get(index).copied().unwrap_or(self.foreground)
            }
        }
    }

    /// SGR 2 (faint): blend two thirds of the way from the background to `color`.
    pub fn dim(&self, color: Hsla) -> Hsla {
        let c = color.to_rgb();
        let b = self.background.to_rgb();
        let mix = |x: f32, y: f32| y + (x - y) * 0.66;
        Rgba {
            r: mix(c.r, b.r),
            g: mix(c.g, b.g),
            b: mix(c.b, b.b),
            a: c.a,
        }
        .into()
    }
}

/// `Colors::get` that also works for indices out of range.
trait ColorsExt {
    fn get(&self, index: usize) -> Option<Rgb>;
}

impl ColorsExt for Colors {
    fn get(&self, index: usize) -> Option<Rgb> {
        (index < alacritty_terminal::term::color::COUNT)
            .then(|| self[index])
            .flatten()
    }
}

pub fn from_rgb(rgb: Rgb) -> Hsla {
    Rgba {
        r: f32::from(rgb.r) / 255.0,
        g: f32::from(rgb.g) / 255.0,
        b: f32::from(rgb.b) / 255.0,
        a: 1.0,
    }
    .into()
}

pub fn to_rgb(color: Hsla) -> Rgb {
    let c = color.to_rgb();
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    Rgb {
        r: byte(c.r),
        g: byte(c.g),
        b: byte(c.b),
    }
}

pub fn hash_hsla(c: Hsla, h: &mut impl Hasher) {
    for v in [c.h, c.s, c.l, c.a] {
        v.to_bits().hash(h);
    }
}

/// Hash a cell colour attribute (vte's `Color` doesn't implement `Hash`).
pub fn hash_color(c: Color, h: &mut impl Hasher) {
    match c {
        Color::Named(n) => (0u8, n as usize).hash(h),
        Color::Spec(rgb) => (1u8, rgb.r, rgb.g, rgb.b).hash(h),
        Color::Indexed(i) => (2u8, i).hash(h),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn palette(dark: bool) -> Palette {
        let bg = if dark { rgb(0x101010) } else { rgb(0xffffff) };
        let fg = if dark { rgb(0xeeeeee) } else { rgb(0x111111) };
        Palette::new(bg.into(), fg.into(), fg.into(), rgb(0x3366ff).into(), dark)
    }

    #[test]
    fn trm_002_256_colour_cube_and_greys() {
        let p = palette(true);
        assert_eq!(to_rgb(p.indexed(16)), Rgb { r: 0, g: 0, b: 0 });
        assert_eq!(to_rgb(p.indexed(196)), Rgb { r: 255, g: 0, b: 0 });
        assert_eq!(to_rgb(p.indexed(21)), Rgb { r: 0, g: 0, b: 255 });
        assert_eq!(to_rgb(p.indexed(232)), Rgb { r: 8, g: 8, b: 8 });
        assert_eq!(
            to_rgb(p.indexed(255)),
            Rgb {
                r: 238,
                g: 238,
                b: 238
            }
        );
    }

    #[test]
    fn named_and_overrides() {
        let p = palette(false);
        let mut overrides = Colors::default();
        assert_eq!(
            p.resolve(Color::Named(NamedColor::Background), &overrides),
            p.background
        );
        assert_eq!(
            to_rgb(p.resolve(Color::Named(NamedColor::Red), &overrides)),
            Rgb {
                r: 0xcd,
                g: 0x31,
                b: 0x31
            }
        );
        overrides[1] = Some(Rgb { r: 1, g: 2, b: 3 });
        assert_eq!(
            to_rgb(p.resolve(Color::Named(NamedColor::Red), &overrides)),
            Rgb { r: 1, g: 2, b: 3 }
        );
        assert_eq!(p.query(1, &overrides), Rgb { r: 1, g: 2, b: 3 });
        assert_ne!(p.key(&overrides), p.key(&Colors::default()));
        // Light and dark variants differ.
        assert_ne!(palette(true).indexed(2), palette(false).indexed(2));
    }
}
