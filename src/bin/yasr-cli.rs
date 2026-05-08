use anyhow::{bail, Context, Result};
use clap::Parser;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use xcap::Monitor;
use yasr::{capture, encode, vaapi};

#[derive(Parser)]
#[command(version, about = "Yet Another Screen Recorder")]
struct Args {
    #[arg(default_value = "output.webm")]
    output: String,

    #[arg(short, long, help = "Monitor index (0-based)")]
    monitor: Option<usize>,

    #[arg(
        short,
        long,
        default_value = "auto",
        help = "Target FPS, or 'auto' to detect"
    )]
    fps: String,

    #[arg(long, help = "Video codec (auto-detected by default)")]
    codec: Option<String>,

    #[arg(short = 'b', long, help = "Video bitrate (e.g. 2M, 500k)")]
    bitrate: Option<String>,

    #[arg(long)]
    list_monitors: bool,

    #[arg(long)]
    list_codecs: bool,

    #[arg(short, long, help = "Show verbose ffmpeg output")]
    verbose: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let monitors = Monitor::all().context(
        "Failed to enumerate monitors. On Wayland, make sure xdg-desktop-portal is running.",
    )?;

    if args.list_monitors {
        return capture::list_monitors(&monitors);
    }

    let available = encode::detect_encoders().context("ffmpeg not found; install ffmpeg")?;

    if args.list_codecs {
        encode::list_codecs(&available);
        return Ok(());
    }

    if monitors.is_empty() {
        bail!("No monitors found");
    }

    let idx = args.monitor.unwrap_or(0);
    let mon = monitors
        .get(idx)
        .with_context(|| format!("Monitor {idx} not found ({} available)", monitors.len()))?;

    let width = mon.width()?;
    let height = mon.height()?;
    let name = mon.name().unwrap_or_else(|_| format!("monitor {idx}"));

    println!("Monitor: {name} ({width}x{height})");

    let codec = args
        .codec
        .clone()
        .unwrap_or_else(|| encode::pick_best(&available));
    println!("Encoder: {codec}");

    let target_fps = if args.fps == "auto" {
        let cap_fps = capture::probe_fps(mon);
        let fps = (cap_fps as u32).clamp(1, 30);
        println!("Capture: {cap_fps:.1} fps max → using {fps} fps");
        fps
    } else {
        args.fps
            .parse::<u32>()
            .context("--fps must be a number or 'auto'")?
    };

    if codec.ends_with("_vaapi") {
        vaapi::setup_env();
    }

    let ffmpeg_args = encode::build_args(
        width,
        height,
        target_fps,
        &codec,
        args.bitrate.as_deref(),
        &args.output,
        args.verbose,
    );
    eprintln!("+ ffmpeg {}", ffmpeg_args.join(" "));

    let mut child = Command::new("ffmpeg")
        .args(&ffmpeg_args)
        .stdin(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .context("failed to spawn ffmpeg")?;
    let mut stdin = child.stdin.take().context("no stdin on ffmpeg")?;

    let running = Arc::new(AtomicBool::new(true));
    let sig = running.clone();
    ctrlc::set_handler(move || sig.store(false, Ordering::SeqCst))
        .context("failed to set Ctrl+C handler")?;

    let interval = Duration::from_secs_f64(1.0 / target_fps as f64);
    let start = Instant::now();
    let mut frames = 0u64;
    let mut last_status = Instant::now();

    println!("Recording... Ctrl+C to stop");

    while running.load(Ordering::SeqCst) {
        let frame_start = Instant::now();
        let img = mon.capture_image().context("capture failed")?;
        stdin.write_all(&img).context("write to ffmpeg failed")?;
        frames += 1;

        if let Some(sleep) = interval.checked_sub(frame_start.elapsed()) {
            std::thread::sleep(sleep);
        }

        if last_status.elapsed() >= Duration::from_secs(1) {
            print!("\r  {}s  {frames} frames", start.elapsed().as_secs());
            std::io::stdout().flush().ok();
            last_status = Instant::now();
        }
    }

    println!("\nFinishing...");
    drop(stdin);
    let status = child.wait()?;

    let elapsed = start.elapsed().as_secs_f64();
    let actual_fps = frames as f64 / elapsed;
    println!(
        "Done: {frames} frames in {elapsed:.1}s ({actual_fps:.1} fps) → {}",
        args.output
    );

    if !status.success() {
        eprintln!("ffmpeg exited with: {status}");
    }

    Ok(())
}
