use anyhow::{Context, Result, anyhow};
use image::{DynamicImage, GenericImageView, Rgba, RgbaImage};
use serde::Deserialize;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    input: PathBuf,
}

#[derive(Debug)]
struct Settings {
    input: PathBuf,
    output: PathBuf,
}

#[derive(Debug)]
struct ValidFrame {
    src_x: u32,
    src_y: u32,
    src_w: u32,
    src_h: u32,
    row: u32,
    col: u32,
}

fn main() -> Result<()> {
    let settings = Settings::from_env()?;
    let source_img = load_input_image(&settings)?;
    let actual_frames = detect_frames(&source_img)?;
    let (canvas_w, canvas_h) = resolve_canvas(&actual_frames)?;

    let columns = actual_frames
        .iter()
        .map(|frame| frame.col + 1)
        .max()
        .unwrap_or(1);
    let rows = actual_frames
        .iter()
        .map(|frame| frame.row + 1)
        .max()
        .unwrap_or(1);

    let mut clean_sheet =
        RgbaImage::from_pixel(canvas_w * columns, canvas_h * rows, Rgba([0, 0, 0, 0]));

    for frame in &actual_frames {
        validate_frame_bounds(&source_img, frame)?;

        let cropped = source_img
            .crop_imm(frame.src_x, frame.src_y, frame.src_w, frame.src_h)
            .to_rgba8();

        let mut normalized = RgbaImage::from_pixel(canvas_w, canvas_h, Rgba([0, 0, 0, 0]));
        blit_clipped(&mut normalized, &cropped, 0, 0);

        let sheet_x = frame.col * canvas_w;
        let sheet_y = frame.row * canvas_h;

        blit_clipped(
            &mut clean_sheet,
            &normalized,
            sheet_x as i32,
            sheet_y as i32,
        );
    }

    ensure_parent_dir(&settings.output)?;
    clean_sheet.save(&settings.output)?;

    println!("OK: wrote {}", settings.output.display());
    Ok(())
}

impl Settings {
    fn from_env() -> Result<Self> {
        let config_path = parse_config_path()?;
        let config = read_config(&config_path)?;

        Ok(Self {
            input: config.input,
            output: default_output_path(&config_path),
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

fn default_output_path(config_path: &Path) -> PathBuf {
    let stem = config_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("sheet");
    PathBuf::from("dist").join(format!("{}.png", stem))
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

fn detect_frames(source_img: &DynamicImage) -> Result<Vec<ValidFrame>> {
    let (img_w, img_h) = source_img.dimensions();
    let y_runs = detect_alpha_axis_runs(source_img, Axis::Y, 0, img_w, 0, img_h);
    let mut frames = Vec::new();

    for (row_index, (src_y, end_y)) in y_runs.iter().copied().enumerate() {
        let x_runs = detect_alpha_axis_runs(source_img, Axis::X, 0, img_w, src_y, end_y);
        for (col_index, (src_x, end_x)) in x_runs.into_iter().enumerate() {
            frames.push(ValidFrame {
                src_x,
                src_y,
                src_w: end_x - src_x,
                src_h: end_y - src_y,
                row: row_index as u32,
                col: col_index as u32,
            });
        }
    }

    if frames.is_empty() {
        return Err(anyhow!("auto detection found no non-transparent pixels"));
    }

    Ok(frames)
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

fn resolve_canvas(frames: &[ValidFrame]) -> Result<(u32, u32)> {
    let canvas_w = frames.iter().map(|frame| frame.src_w).max().unwrap_or(0);
    let canvas_h = frames.iter().map(|frame| frame.src_h).max().unwrap_or(0);

    if canvas_w == 0 || canvas_h == 0 {
        return Err(anyhow!("detected frames have zero-sized canvas"));
    }

    Ok((canvas_w, canvas_h))
}

fn validate_frame_bounds(img: &DynamicImage, frame: &ValidFrame) -> Result<()> {
    let (img_w, img_h) = img.dimensions();
    let end_x = frame.src_x.saturating_add(frame.src_w);
    let end_y = frame.src_y.saturating_add(frame.src_h);

    if frame.src_w == 0 || frame.src_h == 0 {
        return Err(anyhow!("frame has zero size: {:?}", frame));
    }

    if end_x > img_w || end_y > img_h {
        return Err(anyhow!(
            "frame out of bounds: src=({}, {}) size={}x{}, image={}x{}",
            frame.src_x,
            frame.src_y,
            frame.src_w,
            frame.src_h,
            img_w,
            img_h
        ));
    }

    Ok(())
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
