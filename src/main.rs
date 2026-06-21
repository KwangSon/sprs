use anyhow::{Context, Result, anyhow};
use clap::Parser;
use gif::{DisposalMethod, Encoder, Frame as GifFrame, Repeat};
use image::{DynamicImage, GenericImageView, Rgba, RgbaImage};
use serde::Deserialize;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Parser)]
#[command(name = "sprs")]
#[command(about = "Normalize irregular sprite frames by aligning their pivot points.")]
struct Args {
    #[arg(long)]
    config: Option<PathBuf>,

    #[arg(long, conflicts_with = "input_url")]
    input: Option<PathBuf>,

    #[arg(long, conflicts_with = "input")]
    input_url: Option<String>,

    #[arg(long)]
    frames: Option<PathBuf>,

    #[arg(long)]
    canvas: Option<String>,

    #[arg(long)]
    target_pivot: Option<String>,

    #[arg(long)]
    output: Option<PathBuf>,

    #[arg(long)]
    frames_out_dir: Option<PathBuf>,

    #[arg(long)]
    pivot_png_dir: Option<PathBuf>,

    #[arg(long)]
    debug_sheet: Option<PathBuf>,

    #[arg(long)]
    gif: Option<PathBuf>,

    #[arg(long)]
    fps: Option<u32>,

    #[arg(long)]
    columns: Option<u32>,

    #[arg(long)]
    marker_size: Option<i32>,

    #[arg(long)]
    gif_scale: Option<u32>,

    #[arg(long)]
    out_meta: Option<PathBuf>,
}

#[derive(Debug, Default, Deserialize)]
struct ConfigFile {
    input: Option<PathBuf>,
    input_url: Option<String>,
    frames: Option<PathBuf>,
    canvas: Option<String>,
    target_pivot: Option<String>,
    output: Option<PathBuf>,
    frames_out_dir: Option<PathBuf>,
    pivot_png_dir: Option<PathBuf>,
    debug_sheet: Option<PathBuf>,
    gif: Option<PathBuf>,
    fps: Option<u32>,
    columns: Option<u32>,
    marker_size: Option<i32>,
    gif_scale: Option<u32>,
    out_meta: Option<PathBuf>,
}

#[derive(Debug)]
struct Settings {
    input: Option<PathBuf>,
    input_url: Option<String>,
    frames: PathBuf,
    canvas: String,
    target_pivot: Option<String>,
    output: PathBuf,
    frames_out_dir: Option<PathBuf>,
    pivot_png_dir: Option<PathBuf>,
    debug_sheet: Option<PathBuf>,
    gif: Option<PathBuf>,
    fps: u32,
    columns: Option<u32>,
    marker_size: i32,
    gif_scale: u32,
    out_meta: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
struct FramesFile {
    y_coords: Vec<u32>,
    default_pivot: Option<[i32; 2]>,
    rows: Vec<RowDef>,
}

#[derive(Debug, Deserialize)]
struct RowDef {
    prefix: String,
    x_coords: Vec<u32>,
    pivots: Option<Vec<[i32; 2]>>,
}

#[derive(Debug)]
struct ValidFrame {
    name: String,
    src_x: u32,
    src_y: u32,
    src_w: u32,
    src_h: u32,
    pivot_x: i32,
    pivot_y: i32,
}

#[derive(Debug, serde::Serialize)]
struct OutputMeta {
    canvas_w: u32,
    canvas_h: u32,
    target_pivot_x: i32,
    target_pivot_y: i32,
    columns: u32,
    rows: u32,
    output_frames: Vec<OutputFrameMeta>,
}

#[derive(Debug, serde::Serialize)]
struct OutputFrameMeta {
    name: String,
    src_x: u32,
    src_y: u32,
    src_w: u32,
    src_h: u32,
    pivot_x: i32,
    pivot_y: i32,
    dst_x: i32,
    dst_y: i32,
    sheet_x: u32,
    sheet_y: u32,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let settings = Settings::from_args(args)?;

    let (canvas_w, canvas_h) = parse_size(&settings.canvas)?;
    let frames_file = read_frames(&settings.frames)?;

    if frames_file.rows.is_empty() {
        return Err(anyhow!("frames.json has no rows"));
    }

    if frames_file.y_coords.len() < frames_file.rows.len() + 1 {
        return Err(anyhow!("frames.json y_coords length must be at least rows.len() + 1"));
    }

    let (target_pivot_x, target_pivot_y) = match &settings.target_pivot {
        Some(p) => parse_point(p)?,
        None => {
            if let Some(dp) = frames_file.default_pivot {
                (dp[0], dp[1])
            } else if let Some(first_row) = frames_file.rows.first() {
                if let Some(pivots) = &first_row.pivots {
                    if let Some(p) = pivots.first() {
                        (p[0], p[1])
                    } else {
                        return Err(anyhow!("no pivot found in first row"));
                    }
                } else {
                    return Err(anyhow!("no default_pivot and no pivots in first row"));
                }
            } else {
                return Err(anyhow!("frames.json has no valid rows to extract pivot"));
            }
        }
    };

    let source_img = load_input_image(&settings)?;

    let mut actual_frames = Vec::new();
    for (r, row) in frames_file.rows.iter().enumerate() {
        if row.x_coords.len() < 2 {
            continue;
        }

        let src_y = frames_file.y_coords[r];
        let src_h = frames_file.y_coords[r + 1].saturating_sub(src_y);
        let num_frames = row.x_coords.len() - 1;

        if let Some(cols) = settings.columns {
            if num_frames as u32 != cols {
                println!(
                    "Warning: row '{}' has {} frames, but settings.columns is {}",
                    row.prefix, num_frames, cols
                );
            }
        }

        for c in 0..num_frames {
            let src_x = row.x_coords[c];
            let src_w = row.x_coords[c + 1].saturating_sub(src_x);

            let pivot = if let Some(pivots) = &row.pivots {
                if let Some(p) = pivots.get(c) {
                    *p
                } else {
                    frames_file.default_pivot.unwrap_or([0, 0])
                }
            } else {
                frames_file.default_pivot.unwrap_or([0, 0])
            };

            actual_frames.push(ValidFrame {
                name: format!("{}_{:02}", row.prefix, c),
                src_x,
                src_y,
                src_w,
                src_h,
                pivot_x: pivot[0],
                pivot_y: pivot[1],
            });
        }
    }

    if actual_frames.is_empty() {
        return Err(anyhow!("frames.json has no valid frames"));
    }

    let columns = settings
        .columns
        .unwrap_or(actual_frames.len() as u32)
        .max(1);
    let rows = ((actual_frames.len() as u32) + columns - 1) / columns;

    let mut clean_sheet =
        RgbaImage::from_pixel(canvas_w * columns, canvas_h * rows, Rgba([0, 0, 0, 0]));

    let mut debug_sheet =
        RgbaImage::from_pixel(canvas_w * columns, canvas_h * rows, Rgba([0, 0, 0, 0]));

    if let Some(dir) = &settings.frames_out_dir {
        fs::create_dir_all(dir)?;
    }

    if let Some(dir) = &settings.pivot_png_dir {
        fs::create_dir_all(dir)?;
    }

    let mut normalized_frames = Vec::new();
    let mut output_meta_frames = Vec::new();

    for (i, frame) in actual_frames.iter().enumerate() {
        validate_frame_bounds(&source_img, frame)?;

        let name = &frame.name;

        let cropped = source_img
            .crop_imm(frame.src_x, frame.src_y, frame.src_w, frame.src_h)
            .to_rgba8();

        let dst_x = target_pivot_x - frame.pivot_x;
        let dst_y = target_pivot_y - frame.pivot_y;

        let mut normalized = RgbaImage::from_pixel(canvas_w, canvas_h, Rgba([0, 0, 0, 0]));
        blit_clipped(&mut normalized, &cropped, dst_x, dst_y);

        let mut marked = normalized.clone();
        draw_pivot_marker(
            &mut marked,
            target_pivot_x,
            target_pivot_y,
            settings.marker_size,
        );

        let col = (i as u32) % columns;
        let row = (i as u32) / columns;
        let sheet_x = col * canvas_w;
        let sheet_y = row * canvas_h;

        blit_clipped(
            &mut clean_sheet,
            &normalized,
            sheet_x as i32,
            sheet_y as i32,
        );
        blit_clipped(&mut debug_sheet, &marked, sheet_x as i32, sheet_y as i32);

        if let Some(dir) = &settings.frames_out_dir {
            normalized.save(dir.join(format!("{:03}_{}.png", i, sanitize_filename(name))))?;
        }

        if let Some(dir) = &settings.pivot_png_dir {
            marked.save(dir.join(format!("{:03}_{}_pivot.png", i, sanitize_filename(name))))?;
        }

        normalized_frames.push(normalized);

        output_meta_frames.push(OutputFrameMeta {
            name: name.clone(),
            src_x: frame.src_x,
            src_y: frame.src_y,
            src_w: frame.src_w,
            src_h: frame.src_h,
            pivot_x: frame.pivot_x,
            pivot_y: frame.pivot_y,
            dst_x,
            dst_y,
            sheet_x,
            sheet_y,
        });
    }

    ensure_parent_dir(&settings.output)?;
    clean_sheet.save(&settings.output)?;

    if let Some(path) = &settings.debug_sheet {
        ensure_parent_dir(path)?;
        debug_sheet.save(path)?;
    }

    if let Some(path) = &settings.gif {
        ensure_parent_dir(path)?;
        write_gif_preview(path, &normalized_frames, settings.fps, settings.gif_scale)?;
    }

    if let Some(path) = &settings.out_meta {
        ensure_parent_dir(path)?;
        let meta = OutputMeta {
            canvas_w,
            canvas_h,
            target_pivot_x,
            target_pivot_y,
            columns,
            rows,
            output_frames: output_meta_frames,
        };
        fs::write(path, serde_json::to_string_pretty(&meta)?)?;
    }

    println!("OK: wrote {}", settings.output.display());
    Ok(())
}

impl Settings {
    fn from_args(args: Args) -> Result<Self> {
        let config = match &args.config {
            Some(path) => read_config(path)?,
            None => ConfigFile::default(),
        };

        let (input, input_url) = if args.input.is_some() || args.input_url.is_some() {
            (args.input, args.input_url)
        } else {
            (config.input, config.input_url)
        };

        if input.is_some() == input_url.is_some() {
            return Err(anyhow!(
                "provide exactly one input source: --input/input or --input-url/input_url"
            ));
        }

        Ok(Self {
            input,
            input_url,
            frames: args
                .frames
                .or(config.frames)
                .context("missing frames; pass --frames or set frames in config")?,
            canvas: args
                .canvas
                .or(config.canvas)
                .context("missing canvas; pass --canvas or set canvas in config")?,
            target_pivot: args.target_pivot.or(config.target_pivot),
            output: args
                .output
                .or(config.output)
                .context("missing output; pass --output or set output in config")?,
            frames_out_dir: args.frames_out_dir.or(config.frames_out_dir),
            pivot_png_dir: args.pivot_png_dir.or(config.pivot_png_dir),
            debug_sheet: args.debug_sheet.or(config.debug_sheet),
            gif: args.gif.or(config.gif),
            fps: args.fps.or(config.fps).unwrap_or(8),
            columns: args.columns.or(config.columns),
            marker_size: args.marker_size.or(config.marker_size).unwrap_or(4),
            gif_scale: args.gif_scale.or(config.gif_scale).unwrap_or(1),
            out_meta: args.out_meta.or(config.out_meta),
        })
    }
}

fn load_input_image(settings: &Settings) -> Result<DynamicImage> {
    if let Some(path) = &settings.input {
        return image::open(path).with_context(|| format!("failed to open {}", path.display()));
    }

    if let Some(url) = &settings.input_url {
        let bytes = reqwest::blocking::get(url)
            .with_context(|| format!("failed to GET {}", url))?
            .bytes()
            .context("failed to read URL response bytes")?;

        return image::load_from_memory(&bytes).context("failed to decode image from URL");
    }

    Err(anyhow!("provide either --input or --input-url"))
}

fn read_config(path: &Path) -> Result<ConfigFile> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("failed to read config {}", path.display()))?;
    serde_json::from_str(&text)
        .with_context(|| format!("failed to parse config {}", path.display()))
}

fn read_frames(path: &Path) -> Result<FramesFile> {
    let text = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&text)?)
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

fn parse_size(text: &str) -> Result<(u32, u32)> {
    let parts: Vec<&str> = text.split('x').collect();
    if parts.len() != 2 {
        return Err(anyhow!("invalid size '{}', expected WIDTHxHEIGHT", text));
    }

    let w = parts[0].parse::<u32>()?;
    let h = parts[1].parse::<u32>()?;

    if w == 0 || h == 0 {
        return Err(anyhow!("canvas size must be non-zero"));
    }

    Ok((w, h))
}

fn parse_point(text: &str) -> Result<(i32, i32)> {
    let parts: Vec<&str> = text.split(',').collect();
    if parts.len() != 2 {
        return Err(anyhow!("invalid point '{}', expected X,Y", text));
    }

    Ok((parts[0].parse::<i32>()?, parts[1].parse::<i32>()?))
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

fn draw_pivot_marker(img: &mut RgbaImage, x: i32, y: i32, size: i32) {
    let red = Rgba([255, 0, 0, 255]);
    let yellow = Rgba([255, 255, 0, 255]);

    for d in -size..=size {
        set_pixel_safe(img, x + d, y, red);
        set_pixel_safe(img, x, y + d, red);
    }

    set_pixel_safe(img, x, y, yellow);
    set_pixel_safe(img, x - 1, y, yellow);
    set_pixel_safe(img, x + 1, y, yellow);
    set_pixel_safe(img, x, y - 1, yellow);
    set_pixel_safe(img, x, y + 1, yellow);
}

fn set_pixel_safe(img: &mut RgbaImage, x: i32, y: i32, color: Rgba<u8>) {
    if x >= 0 && y >= 0 && x < img.width() as i32 && y < img.height() as i32 {
        img.put_pixel(x as u32, y as u32, color);
    }
}

fn write_gif_preview(path: &Path, frames: &[RgbaImage], fps: u32, scale: u32) -> Result<()> {
    if frames.is_empty() {
        return Err(anyhow!("no frames for GIF"));
    }

    let delay = (100.0 / fps.max(1) as f32).round().max(1.0) as u16;
    let scale = scale.max(1);

    let base_w = frames[0].width();
    let base_h = frames[0].height();
    let gif_w = base_w * scale;
    let gif_h = base_h * scale;

    let mut file = File::create(path)?;
    let mut encoder = Encoder::new(&mut file, gif_w as u16, gif_h as u16, &[])?;
    encoder.set_repeat(Repeat::Infinite)?;

    for img in frames {
        let frame_img = if scale == 1 {
            img.clone()
        } else {
            image::imageops::resize(img, gif_w, gif_h, image::imageops::FilterType::Nearest)
        };

        let mut raw = frame_img.into_raw();
        let mut frame = GifFrame::from_rgba_speed(gif_w as u16, gif_h as u16, &mut raw, 10);
        frame.delay = delay;
        frame.dispose = DisposalMethod::Background;
        encoder.write_frame(&frame)?;
    }

    drop(encoder);
    file.flush()?;

    Ok(())
}

fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }

    Ok(())
}

fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}
