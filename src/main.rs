use anyhow::{Context, Result, anyhow};
use gif::{DisposalMethod, Encoder, Frame as GifFrame, Repeat};
use image::{DynamicImage, GenericImageView, Rgba, RgbaImage};
use serde::Deserialize;
use std::env;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

const GIF_FPS: u32 = 5;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    input: PathBuf,
    name: Option<Vec<String>>,
}

#[derive(Debug)]
struct Settings {
    input: PathBuf,
    output_stem: String,
    names: Option<Vec<String>>,
}

#[derive(Debug)]
struct RowRegion {
    src_y: u32,
    src_h: u32,
}

fn main() -> Result<()> {
    let settings = Settings::from_env()?;
    let source_img = load_input_image(&settings)?;
    let rows = detect_rows(&source_img)?;
    validate_names(&settings.names, rows.len())?;

    for (i, row) in rows.iter().enumerate() {
        validate_row_bounds(&source_img, row)?;
        let x_runs = detect_frame_runs(&source_img, row)?;
        let output_name = row_output_name(&settings, i);

        let output = row_output_path(&output_name);
        ensure_parent_dir(&output)?;
        write_row_sheet_png(&output, &source_img, row, &x_runs)?;

        println!("OK: wrote {}", output.display());

        let debug_output = row_debug_output_path(&output_name);
        ensure_parent_dir(&debug_output)?;
        write_row_debug_png(&debug_output, &source_img, row, &x_runs)?;

        println!("OK: wrote {}", debug_output.display());

        let gif_output = row_gif_output_path(&output_name);
        ensure_parent_dir(&gif_output)?;
        write_row_gif(&gif_output, &source_img, row, &x_runs, GIF_FPS)?;

        println!("OK: wrote {}", gif_output.display());
    }

    Ok(())
}

impl Settings {
    fn from_env() -> Result<Self> {
        let config_path = parse_config_path()?;
        let config = read_config(&config_path)?;

        Ok(Self {
            input: config.input,
            output_stem: config_stem(&config_path),
            names: config.name,
        })
    }
}

fn parse_config_path() -> Result<PathBuf> {
    let mut args = env::args_os();
    let program = args.next().unwrap_or_default();
    let Some(config_path) = args.next() else {
        return Err(anyhow!(
            "missing config path; usage: {} <config.json>",
            Path::new(&program).display()
        ));
    };

    if args.next().is_some() {
        return Err(anyhow!(
            "expected exactly one config path; usage: {} <config.json>",
            Path::new(&program).display()
        ));
    }

    Ok(PathBuf::from(config_path))
}

fn config_stem(config_path: &Path) -> String {
    config_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("sheet")
        .to_owned()
}

fn validate_names(names: &Option<Vec<String>>, row_count: usize) -> Result<()> {
    if let Some(names) = names {
        if names.len() != row_count {
            return Err(anyhow!(
                "name length must match detected row count: names={}, rows={}",
                names.len(),
                row_count
            ));
        }
    }

    Ok(())
}

fn row_output_name(settings: &Settings, row_index: usize) -> String {
    let suffix = settings
        .names
        .as_ref()
        .and_then(|names| names.get(row_index))
        .cloned()
        .unwrap_or_else(|| format!("r{}", row_index));

    format!("{}_{}", settings.output_stem, suffix)
}

fn row_output_path(name: &str) -> PathBuf {
    PathBuf::from("prd").join(format!("{}.png", name))
}

fn row_debug_output_path(name: &str) -> PathBuf {
    PathBuf::from("debug").join(format!("{}.png", name))
}

fn row_gif_output_path(name: &str) -> PathBuf {
    PathBuf::from("debug").join(format!("{}.gif", name))
}

fn load_input_image(settings: &Settings) -> Result<DynamicImage> {
    image::open(&settings.input)
        .with_context(|| format!("failed to open {}", settings.input.display()))
}

fn read_config(path: &Path) -> Result<ConfigFile> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("failed to read config {}", path.display()))?;
    serde_json::from_str(&text)
        .with_context(|| format!("failed to parse config {}", path.display()))
}

fn detect_rows(source_img: &DynamicImage) -> Result<Vec<RowRegion>> {
    let (img_w, img_h) = source_img.dimensions();
    let y_runs = detect_alpha_axis_runs(source_img, Axis::Y, 0, img_w, 0, img_h);

    if y_runs.is_empty() {
        return Err(anyhow!("auto detection found no non-transparent pixels"));
    }

    Ok(y_runs
        .into_iter()
        .map(|(src_y, end_y)| RowRegion {
            src_y,
            src_h: end_y - src_y,
        })
        .collect())
}

#[derive(Clone, Copy)]
enum Axis {
    X,
    Y,
}

fn detect_alpha_axis_runs(
    source_img: &DynamicImage,
    axis: Axis,
    start_x: u32,
    end_x: u32,
    start_y: u32,
    end_y: u32,
) -> Vec<(u32, u32)> {
    let mut runs = Vec::new();
    let mut run_start = None;
    let (range_start, range_end) = match axis {
        Axis::X => (start_x, end_x),
        Axis::Y => (start_y, end_y),
    };

    for pos in range_start..range_end {
        let occupied = match axis {
            Axis::X => (start_y..end_y).any(|y| source_img.get_pixel(pos, y)[3] > 0),
            Axis::Y => (start_x..end_x).any(|x| source_img.get_pixel(x, pos)[3] > 0),
        };

        match (run_start, occupied) {
            (None, true) => run_start = Some(pos),
            (Some(start), false) => {
                runs.push((start, pos));
                run_start = None;
            }
            _ => {}
        }
    }

    if let Some(start) = run_start {
        runs.push((start, range_end));
    }

    runs
}

fn validate_row_bounds(img: &DynamicImage, row: &RowRegion) -> Result<()> {
    let (_, img_h) = img.dimensions();
    let end_y = row.src_y.saturating_add(row.src_h);

    if row.src_h == 0 {
        return Err(anyhow!("row has zero height: {:?}", row));
    }

    if end_y > img_h {
        return Err(anyhow!(
            "row out of bounds: y={} height={}, image height={}",
            row.src_y,
            row.src_h,
            img_h
        ));
    }

    Ok(())
}

fn detect_frame_runs(source_img: &DynamicImage, row: &RowRegion) -> Result<Vec<(u32, u32)>> {
    let (img_w, _) = source_img.dimensions();
    let x_runs = detect_alpha_axis_runs(
        source_img,
        Axis::X,
        0,
        img_w,
        row.src_y,
        row.src_y + row.src_h,
    );

    if x_runs.is_empty() {
        return Err(anyhow!(
            "row has no non-transparent frame pixels: {:?}",
            row
        ));
    }

    Ok(x_runs)
}

fn write_row_sheet_png(
    path: &Path,
    source_img: &DynamicImage,
    row: &RowRegion,
    x_runs: &[(u32, u32)],
) -> Result<()> {
    let (canvas_w, canvas_h) = row_canvas_size(row, x_runs)?;
    let mut sheet =
        RgbaImage::from_pixel(canvas_w * x_runs.len() as u32, canvas_h, Rgba([0, 0, 0, 0]));

    for (i, (start_x, end_x)) in x_runs.iter().copied().enumerate() {
        let frame_w = end_x - start_x;
        let cropped = source_img
            .crop_imm(start_x, row.src_y, frame_w, row.src_h)
            .to_rgba8();
        blit_clipped(&mut sheet, &cropped, (i as u32 * canvas_w) as i32, 0);
    }

    sheet.save(path)?;
    Ok(())
}

fn write_row_debug_png(
    path: &Path,
    source_img: &DynamicImage,
    row: &RowRegion,
    x_runs: &[(u32, u32)],
) -> Result<()> {
    let (canvas_w, canvas_h) = row_canvas_size(row, x_runs)?;
    let output_w = canvas_w * x_runs.len() as u32 + x_runs.len().saturating_sub(1) as u32;
    let mut debug = RgbaImage::from_pixel(output_w, canvas_h, Rgba([0, 0, 0, 0]));

    for (i, (start_x, end_x)) in x_runs.iter().copied().enumerate() {
        let frame_w = end_x - start_x;
        let cropped = source_img
            .crop_imm(start_x, row.src_y, frame_w, row.src_h)
            .to_rgba8();
        let dst_x = i as u32 * (canvas_w + 1);
        blit_clipped(&mut debug, &cropped, dst_x as i32, 0);

        if i + 1 < x_runs.len() {
            draw_vertical_line(&mut debug, dst_x + canvas_w, Rgba([255, 0, 0, 255]));
        }
    }

    debug.save(path)?;
    Ok(())
}

fn write_row_gif(
    path: &Path,
    source_img: &DynamicImage,
    row: &RowRegion,
    x_runs: &[(u32, u32)],
    fps: u32,
) -> Result<()> {
    let (canvas_w, canvas_h) = row_canvas_size(row, x_runs)?;
    let delay = (100.0 / fps.max(1) as f32).round().max(1.0) as u16;
    let mut file = File::create(path)?;
    let mut encoder = Encoder::new(&mut file, canvas_w as u16, canvas_h as u16, &[])?;
    encoder.set_repeat(Repeat::Infinite)?;

    for (start_x, end_x) in x_runs.iter().copied() {
        let frame_w = end_x - start_x;
        let cropped = source_img
            .crop_imm(start_x, row.src_y, frame_w, row.src_h)
            .to_rgba8();
        let mut normalized = RgbaImage::from_pixel(canvas_w, canvas_h, Rgba([0, 0, 0, 0]));
        blit_clipped(&mut normalized, &cropped, 0, 0);

        let mut raw = normalized.into_raw();
        let mut frame = GifFrame::from_rgba_speed(canvas_w as u16, canvas_h as u16, &mut raw, 10);
        frame.delay = delay;
        frame.dispose = DisposalMethod::Background;
        encoder.write_frame(&frame)?;
    }

    drop(encoder);
    file.flush()?;

    Ok(())
}

fn row_canvas_size(row: &RowRegion, x_runs: &[(u32, u32)]) -> Result<(u32, u32)> {
    let canvas_w = x_runs
        .iter()
        .map(|(start_x, end_x)| end_x - start_x)
        .max()
        .unwrap_or(0);
    let canvas_h = row.src_h;

    if canvas_w == 0 || canvas_h == 0 {
        return Err(anyhow!("row has zero-sized canvas: {:?}", row));
    }

    Ok((canvas_w, canvas_h))
}

fn draw_vertical_line(img: &mut RgbaImage, x: u32, color: Rgba<u8>) {
    if x >= img.width() {
        return;
    }

    for y in 0..img.height() {
        img.put_pixel(x, y, color);
    }
}

fn blit_clipped(dst: &mut RgbaImage, src: &RgbaImage, dst_x: i32, dst_y: i32) {
    let dst_w = dst.width() as i32;
    let dst_h = dst.height() as i32;

    for sy in 0..src.height() as i32 {
        for sx in 0..src.width() as i32 {
            let tx = dst_x + sx;
            let ty = dst_y + sy;

            if tx < 0 || ty < 0 || tx >= dst_w || ty >= dst_h {
                continue;
            }

            let px = src.get_pixel(sx as u32, sy as u32);
            if px[3] == 0 {
                continue;
            }

            dst.put_pixel(tx as u32, ty as u32, *px);
        }
    }
}

fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }

    Ok(())
}
