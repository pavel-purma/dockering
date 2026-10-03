//! `cargo xtask icons [--check] [--social]` (REL-050).
//!
//! The SVG files under `assets/app-icon/src` and `assets/brand/src` are the masters. All normal
//! outputs are deterministic and checked byte-for-byte by `--check`. The social preview uses
//! system fonts, so it is generated only when explicitly requested and is excluded from the check.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use resvg::{tiny_skia, usvg};

use crate::util::{Args, workspace_root};

const PNG_SIZES: &[u32] = &[16, 24, 32, 48, 64, 128, 256, 512, 1024];
const ICO_SIZES: &[u32] = &[16, 20, 24, 32, 40, 48, 64, 256];
const WIZARD_SCALES: &[u32] = &[100, 125, 150, 175, 200];
const ICNS_ENTRIES: &[([u8; 4], u32)] = &[
    (*b"icp4", 16),
    (*b"icp5", 32),
    (*b"icp6", 64),
    (*b"ic07", 128),
    (*b"ic08", 256),
    (*b"ic09", 512),
    (*b"ic10", 1024),
    (*b"ic11", 32),
    (*b"ic12", 64),
    (*b"ic13", 256),
    (*b"ic14", 512),
];

struct GeneratedAsset {
    path: PathBuf,
    bytes: Vec<u8>,
}

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let parsed = Args::new(args);
    parsed.reject_unknown(&[], &["--check", "--social"])?;

    let check = parsed.flag("--check");
    let social = parsed.flag("--social") && !check;
    let root = workspace_root();
    let assets = generate(&root, social)?;

    if check {
        check_assets(&root, &assets)
    } else {
        write_assets(&root, &assets)
    }
}

fn generate(root: &Path, social: bool) -> anyhow::Result<Vec<GeneratedAsset>> {
    let app_svg = read(root, "assets/app-icon/src/icon.svg")?;
    let small_svg = read(root, "assets/app-icon/src/icon-small.svg")?;
    let macos_svg = read(root, "assets/app-icon/src/icon-macos.svg")?;

    let app = parse_svg(&app_svg, false).context("failed to parse icon.svg")?;
    let small = parse_svg(&small_svg, false).context("failed to parse icon-small.svg")?;
    let macos = parse_svg(&macos_svg, false).context("failed to parse icon-macos.svg")?;

    let mut assets = Vec::new();
    for &size in PNG_SIZES {
        let tree = if size <= 24 { &small } else { &app };
        assets.push(asset(
            root,
            format!("assets/app-icon/icon-{size}.png"),
            render_png(tree, size, size)?,
        ));
    }

    let mut ico_images = Vec::with_capacity(ICO_SIZES.len());
    for &size in ICO_SIZES {
        let tree = if size <= 24 { &small } else { &app };
        ico_images.push((size, render_png(tree, size, size)?));
    }
    assets.push(asset(
        root,
        "assets/app-icon/icon.ico",
        encode_ico(&ico_images)?,
    ));

    let mut icns_images = Vec::with_capacity(ICNS_ENTRIES.len());
    for &(kind, size) in ICNS_ENTRIES {
        icns_images.push((kind, render_png(&macos, size, size)?));
    }
    assets.push(asset(
        root,
        "assets/app-icon/icon.icns",
        encode_icns(&icns_images)?,
    ));

    for &scale in WIZARD_SCALES {
        let width = scaled_dimension(164, scale);
        let height = scaled_dimension(314, scale);
        let pixmap = render_large_wizard(&app, width, height)?;
        assets.push(asset(
            root,
            format!("packaging/windows/wizard/WizardImage{scale}.bmp"),
            encode_bmp(&pixmap)?,
        ));

        let side = scaled_dimension(55, scale);
        let pixmap = render_small_wizard(&app, side)?;
        assets.push(asset(
            root,
            format!("packaging/windows/wizard/WizardSmallImage{scale}.bmp"),
            encode_bmp(&pixmap)?,
        ));
    }

    assets.push(asset(root, "assets/brand/logo.svg", app_svg));
    assets.push(asset(
        root,
        "assets/brand/logo.png",
        render_png(&app, 256, 256)?,
    ));

    if social {
        let social_svg = read(root, "assets/brand/src/social-preview.svg")?;
        let social_tree =
            parse_svg(&social_svg, true).context("failed to parse social-preview.svg")?;
        assets.push(asset(
            root,
            "assets/brand/social-preview.png",
            render_png(&social_tree, 1280, 640)?,
        ));
    }

    Ok(assets)
}

fn read(root: &Path, relative: &str) -> anyhow::Result<Vec<u8>> {
    let path = root.join(relative);
    fs::read(&path).with_context(|| format!("failed to read {}", display_path(root, &path)))
}

fn asset(root: &Path, relative: impl AsRef<Path>, bytes: Vec<u8>) -> GeneratedAsset {
    GeneratedAsset {
        path: root.join(relative),
        bytes,
    }
}

fn parse_svg(data: &[u8], system_fonts: bool) -> anyhow::Result<usvg::Tree> {
    let mut options = usvg::Options::default();
    if system_fonts {
        options.fontdb_mut().load_system_fonts();
    }
    usvg::Tree::from_data(data, &options).context("invalid SVG")
}

fn render_png(tree: &usvg::Tree, width: u32, height: u32) -> anyhow::Result<Vec<u8>> {
    let mut pixmap = new_pixmap(width, height)?;
    render_fitted(tree, &mut pixmap, 0.0, 0.0, width as f32, height as f32);
    pixmap.encode_png().context("failed to encode PNG")
}

fn render_fitted(
    tree: &usvg::Tree,
    pixmap: &mut tiny_skia::Pixmap,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
) {
    let source = tree.size();
    let scale = (width / source.width()).min(height / source.height());
    let rendered_width = source.width() * scale;
    let rendered_height = source.height() * scale;
    let tx = x + (width - rendered_width) / 2.0;
    let ty = y + (height - rendered_height) / 2.0;
    let transform = tiny_skia::Transform::from_row(scale, 0.0, 0.0, scale, tx, ty);
    resvg::render(tree, transform, &mut pixmap.as_mut());
}

fn render_large_wizard(
    icon: &usvg::Tree,
    width: u32,
    height: u32,
) -> anyhow::Result<tiny_skia::Pixmap> {
    let mut pixmap = new_pixmap(width, height)?;
    fill_vertical_gradient(&mut pixmap, [0x8e, 0x7a, 0xeb], [0x58, 0x40, 0xb5])?;

    let icon_side = width as f32 * 0.45;
    let x = (width as f32 - icon_side) / 2.0;
    let y = height as f32 / 3.0 - icon_side / 2.0;
    render_fitted(icon, &mut pixmap, x, y, icon_side, icon_side);
    Ok(pixmap)
}

fn render_small_wizard(icon: &usvg::Tree, side: u32) -> anyhow::Result<tiny_skia::Pixmap> {
    let mut pixmap = new_pixmap(side, side)?;
    pixmap.fill(tiny_skia::Color::WHITE);
    render_fitted(icon, &mut pixmap, 0.0, 0.0, side as f32, side as f32);
    Ok(pixmap)
}

fn new_pixmap(width: u32, height: u32) -> anyhow::Result<tiny_skia::Pixmap> {
    tiny_skia::Pixmap::new(width, height)
        .with_context(|| format!("invalid raster dimensions {width}x{height}"))
}

fn fill_vertical_gradient(
    pixmap: &mut tiny_skia::Pixmap,
    top: [u8; 3],
    bottom: [u8; 3],
) -> anyhow::Result<()> {
    let width = pixmap.width() as usize;
    let denominator = pixmap.height().saturating_sub(1).max(1);
    for y in 0..pixmap.height() {
        let color = tiny_skia::PremultipliedColorU8::from_rgba(
            lerp_channel(top[0], bottom[0], y, denominator),
            lerp_channel(top[1], bottom[1], y, denominator),
            lerp_channel(top[2], bottom[2], y, denominator),
            255,
        )
        .context("gradient produced an invalid premultiplied color")?;
        let start = y as usize * width;
        pixmap.pixels_mut()[start..start + width].fill(color);
    }
    Ok(())
}

fn lerp_channel(start: u8, end: u8, numerator: u32, denominator: u32) -> u8 {
    let start = i64::from(start);
    let delta = i64::from(end) - start;
    let rounded = if delta >= 0 {
        i64::from(denominator) / 2
    } else {
        -(i64::from(denominator) / 2)
    };
    (start + (delta * i64::from(numerator) + rounded) / i64::from(denominator)) as u8
}

fn scaled_dimension(base: u32, percentage: u32) -> u32 {
    (base * percentage + 50) / 100
}

fn encode_ico(images: &[(u32, Vec<u8>)]) -> anyhow::Result<Vec<u8>> {
    let count = u16::try_from(images.len()).context("too many ICO images")?;
    let directory_len = 6usize
        .checked_add(
            images
                .len()
                .checked_mul(16)
                .context("ICO directory is too large")?,
        )
        .context("ICO directory is too large")?;
    let payload_len = images.iter().try_fold(0usize, |total, (_, png)| {
        total
            .checked_add(png.len())
            .context("ICO payload is too large")
    })?;
    let mut output = Vec::with_capacity(
        directory_len
            .checked_add(payload_len)
            .context("ICO is too large")?,
    );

    push_u16_le(&mut output, 0);
    push_u16_le(&mut output, 1);
    push_u16_le(&mut output, count);

    let mut offset = directory_len;
    for (size, png) in images {
        if *size == 0 || *size > 256 {
            bail!("ICO image size must be between 1 and 256, got {size}");
        }
        output.push(if *size == 256 { 0 } else { *size as u8 });
        output.push(if *size == 256 { 0 } else { *size as u8 });
        output.push(0);
        output.push(0);
        push_u16_le(&mut output, 1);
        push_u16_le(&mut output, 32);
        push_u32_le(
            &mut output,
            u32::try_from(png.len()).context("ICO PNG payload is too large")?,
        );
        push_u32_le(
            &mut output,
            u32::try_from(offset).context("ICO payload offset is too large")?,
        );
        offset = offset
            .checked_add(png.len())
            .context("ICO payload offset is too large")?;
    }
    for (_, png) in images {
        output.extend_from_slice(png);
    }
    Ok(output)
}

fn encode_icns(images: &[([u8; 4], Vec<u8>)]) -> anyhow::Result<Vec<u8>> {
    let body_len = images.iter().try_fold(0usize, |total, (_, png)| {
        total
            .checked_add(8)
            .and_then(|value| value.checked_add(png.len()))
            .context("ICNS is too large")
    })?;
    let total_len = 8usize.checked_add(body_len).context("ICNS is too large")?;
    let mut output = Vec::with_capacity(total_len);
    output.extend_from_slice(b"icns");
    push_u32_be(
        &mut output,
        u32::try_from(total_len).context("ICNS is too large")?,
    );
    for (kind, png) in images {
        output.extend_from_slice(kind);
        push_u32_be(
            &mut output,
            u32::try_from(8usize + png.len()).context("ICNS entry is too large")?,
        );
        output.extend_from_slice(png);
    }
    Ok(output)
}

fn encode_bmp(pixmap: &tiny_skia::Pixmap) -> anyhow::Result<Vec<u8>> {
    let width = pixmap.width();
    let height = pixmap.height();
    let row_bytes = width.checked_mul(3).context("BMP row is too wide")?;
    let stride = row_bytes
        .checked_add(3)
        .map(|value| value & !3)
        .context("BMP row is too wide")?;
    let image_size = stride.checked_mul(height).context("BMP is too large")?;
    let file_size = 54u32.checked_add(image_size).context("BMP is too large")?;
    let mut output = Vec::with_capacity(file_size as usize);

    output.extend_from_slice(b"BM");
    push_u32_le(&mut output, file_size);
    push_u16_le(&mut output, 0);
    push_u16_le(&mut output, 0);
    push_u32_le(&mut output, 54);

    push_u32_le(&mut output, 40);
    output.extend_from_slice(
        &i32::try_from(width)
            .context("BMP width exceeds i32")?
            .to_le_bytes(),
    );
    output.extend_from_slice(
        &i32::try_from(height)
            .context("BMP height exceeds i32")?
            .to_le_bytes(),
    );
    push_u16_le(&mut output, 1);
    push_u16_le(&mut output, 24);
    push_u32_le(&mut output, 0);
    push_u32_le(&mut output, image_size);
    push_u32_le(&mut output, 0);
    push_u32_le(&mut output, 0);
    push_u32_le(&mut output, 0);
    push_u32_le(&mut output, 0);

    let width_usize = width as usize;
    let padding = (stride - row_bytes) as usize;
    for y in (0..height as usize).rev() {
        for pixel in &pixmap.pixels()[y * width_usize..(y + 1) * width_usize] {
            let color = pixel.demultiply();
            output.extend_from_slice(&[color.blue(), color.green(), color.red()]);
        }
        output.resize(output.len() + padding, 0);
    }
    Ok(output)
}

fn check_assets(root: &Path, assets: &[GeneratedAsset]) -> anyhow::Result<()> {
    let mut stale = 0usize;
    for asset in assets {
        match fs::read(&asset.path) {
            Ok(current) if current == asset.bytes => {}
            Ok(_) => {
                eprintln!("stale: {}", display_path(root, &asset.path));
                stale += 1;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                eprintln!("missing: {}", display_path(root, &asset.path));
                stale += 1;
            }
            Err(error) => {
                eprintln!("unreadable: {}: {error}", display_path(root, &asset.path));
                stale += 1;
            }
        }
    }
    if stale != 0 {
        bail!("{stale} generated icon asset(s) are stale; run `cargo xtask icons`");
    }
    println!("icon assets are up to date");
    Ok(())
}

fn write_assets(root: &Path, assets: &[GeneratedAsset]) -> anyhow::Result<()> {
    for asset in assets {
        let parent = asset
            .path
            .parent()
            .context("generated asset path has no parent")?;
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", display_path(root, parent)))?;
        fs::write(&asset.path, &asset.bytes)
            .with_context(|| format!("failed to write {}", display_path(root, &asset.path)))?;
        println!("wrote {}", display_path(root, &asset.path));
    }
    Ok(())
}

fn display_path<'a>(root: &'a Path, path: &'a Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn push_u16_le(output: &mut Vec<u8>, value: u16) {
    output.extend_from_slice(&value.to_le_bytes());
}

fn push_u32_le(output: &mut Vec<u8>, value: u32) {
    output.extend_from_slice(&value.to_le_bytes());
}

fn push_u32_be(output: &mut Vec<u8>, value: u32) {
    output.extend_from_slice(&value.to_be_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rel_050_ico_header_lists_all_sizes() -> anyhow::Result<()> {
        let images = ICO_SIZES
            .iter()
            .map(|size| (*size, vec![0x89, b'P', b'N', b'G', *size as u8]))
            .collect::<Vec<_>>();
        let ico = encode_ico(&images)?;

        assert_eq!(&ico[0..2], &[0, 0]);
        assert_eq!(&ico[2..4], &[1, 0]);
        assert_eq!(u16::from_le_bytes([ico[4], ico[5]]) as usize, images.len());

        let parsed_sizes = (0..images.len())
            .map(|index| {
                let width = ico[6 + index * 16];
                if width == 0 { 256 } else { u32::from(width) }
            })
            .collect::<Vec<_>>();
        assert_eq!(parsed_sizes, ICO_SIZES);

        for (index, (_, payload)) in images.iter().enumerate() {
            let entry = 6 + index * 16;
            let payload_len = u32::from_le_bytes([
                ico[entry + 8],
                ico[entry + 9],
                ico[entry + 10],
                ico[entry + 11],
            ]) as usize;
            let payload_offset = u32::from_le_bytes([
                ico[entry + 12],
                ico[entry + 13],
                ico[entry + 14],
                ico[entry + 15],
            ]) as usize;
            assert_eq!(payload_len, payload.len());
            assert_eq!(&ico[payload_offset..payload_offset + payload_len], payload);
        }
        Ok(())
    }
}
