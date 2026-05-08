use anyhow::{Context, Result, bail};
use clap::Parser;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use xcap::Monitor;

#[derive(Parser)]
#[command(version, about = "Yet Another Screen Recorder")]
struct Args {
    #[arg(default_value = "output.webm")]
    output: String,

    #[arg(short, long, help = "Monitor index (0-based)")]
    monitor: Option<usize>,

    #[arg(short, long, default_value = "auto", help = "Target FPS, or 'auto' to detect")]
    fps: String,

    #[arg(long, help = "Video codec (auto-detected by default)")]
    codec: Option<String>,

    #[arg(short = 'b', long, help = "Video bitrate (e.g. 2M, 500k)")]
    bitrate: Option<String>,

    #[arg(long)]
    list_monitors: bool,

    #[arg(long)]
    list_codecs: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let monitors = Monitor::all()
        .context("Failed to enumerate monitors. On Wayland, make sure xdg-desktop-portal is running.")?;

    if args.list_monitors {
        for (i, m) in monitors.iter().enumerate() {
            let primary = m.is_primary()?;
            println!("[{i}] {} ({}x{} @ {})",
                m.name()?, m.width()?, m.height()?,
                if primary { "primary" } else { "secondary" },
            );
        }
        return Ok(());
    }

    let available = detect_encoders().context("ffmpeg not found; install ffmpeg")?;

    if args.list_codecs {
        println!("Available video encoders:");
        for c in &available {
            println!("  {c}");
        }
        return Ok(());
    }

    if monitors.is_empty() {
        bail!("No monitors found");
    }

    let idx = args.monitor.unwrap_or(0);
    let mon = monitors.get(idx)
        .with_context(|| format!("Monitor {idx} not found ({} available)", monitors.len()))?;

    let width = mon.width()?;
    let height = mon.height()?;
    let name = mon.name().unwrap_or_else(|_| format!("monitor {idx}"));

    println!("Monitor: {name} ({width}x{height})");

    let codec = match &args.codec {
        Some(c) => c.clone(),
        None => pick_best(&available),
    };
    println!("Encoder: {codec}");

    let target_fps = if args.fps == "auto" {
        let cap_fps = probe_capture_fps(mon);
        let fps = (cap_fps as u32).clamp(1, 30);
        println!("Capture: {cap_fps:.1} fps max → using {fps} fps");
        fps
    } else {
        args.fps.parse::<u32>().context("--fps must be a number or 'auto'")?
    };

    let ext = std::path::Path::new(&args.output)
        .extension().and_then(|e| e.to_str()).unwrap_or("webm");

    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-y", "-f", "rawvideo", "-pix_fmt", "bgra",
              "-s", &format!("{width}x{height}"),
              "-r", &target_fps.to_string(), "-i", "-"]);

    if codec.ends_with("_vaapi") {
        setup_vaapi_env();
    }

    match codec.as_str() {
        c if c.ends_with("_vaapi") => {
            cmd.args(["-vaapi_device", vaapi_device().as_str(),
                      "-vf", "format=nv12,hwupload", "-c:v", c]);
        }
        c if c.ends_with("_nvenc") => {
            cmd.args(["-c:v", c, "-pix_fmt", "yuv420p"]);
        }
        "libsvtav1" => {
            cmd.args(["-c:v", "libsvtav1", "-preset", "10",
                      "-svtav1-params", "tune=0:enable-overlays=1"]);
        }
        "libvpx-vp9" => {
            cmd.args(["-c:v", "libvpx-vp9", "-cpu-used", "5",
                      "-deadline", "realtime", "-row-mt", "1"]);
        }
        "libx264" => {
            cmd.args(["-c:v", "libx264", "-preset", "ultrafast",
                      "-tune", "zerolatency"]);
        }
        c => { cmd.args(["-c:v", c]); }
    }

    if let Some(br) = &args.bitrate {
        cmd.args(["-b:v", br]);
    } else {
        let br = match codec.as_str() {
            c if c.contains("vp9") => "2M",
            c if c.contains("av1") => "1.5M",
            c if c.contains("nvenc") => "4M",
            c if c.contains("h264") && !c.contains("vaapi") => "4M",
            _ => "",
        };
        if !br.is_empty() { cmd.args(["-b:v", br]); }
    }

    let fmt = if ext == "webm" { "webm" } else { "mp4" };
    cmd.args(["-f", fmt, &args.output]);

    eprintln!("+ ffmpeg {}", cmd.get_args()
        .map(|a| a.to_string_lossy())
        .collect::<Vec<_>>().join(" "));

    let mut child = cmd.stdin(Stdio::piped()).stderr(Stdio::inherit())
        .spawn().context("failed to spawn ffmpeg")?;
    let mut stdin = child.stdin.take().context("no stdin on ffmpeg")?;

    let running = Arc::new(AtomicBool::new(true));
    let sig = running.clone();
    ctrlc::set_handler(move || sig.store(false, Ordering::SeqCst))
        .context("failed to set Ctrl+C handler")?;

    let interval = Duration::from_secs_f64(1.0 / target_fps as f64);
    let start = Instant::now();
    let mut frames: u64 = 0;
    let mut last_status = Instant::now();

    println!("Recording... Ctrl+C to stop");

    while running.load(Ordering::SeqCst) {
        let frame_start = Instant::now();
        let img = mon.capture_image().context("capture failed")?;
        stdin.write_all(&img).context("write to ffmpeg failed")?;
        frames += 1;

        let elapsed = frame_start.elapsed();
        if let Some(sleep) = interval.checked_sub(elapsed) {
            std::thread::sleep(sleep);
        }

        if last_status.elapsed() >= Duration::from_secs(1) {
            let secs = start.elapsed().as_secs();
            print!("\r  {secs}s  {frames} frames");
            std::io::stdout().flush().ok();
            last_status = Instant::now();
        }
    }

    println!("\nFinishing...");
    drop(stdin);
    let status = child.wait()?;

    let elapsed = start.elapsed().as_secs_f64();
    let actual_fps = frames as f64 / elapsed;
    println!("Done: {frames} frames in {elapsed:.1}s ({actual_fps:.1} fps) → {}",
        args.output);

    if !status.success() {
        eprintln!("ffmpeg exited with: {status}");
    }

    Ok(())
}

fn probe_capture_fps(mon: &Monitor) -> f64 {
    let n = 4;
    let mut elapsed = 0.0f64;
    for i in 0..n {
        let start = Instant::now();
        if mon.capture_image().is_ok() {
            let t = start.elapsed().as_secs_f64();
            if i > 0 { elapsed += t; }
        }
    }
    if elapsed > 0.0 { (n - 1) as f64 / elapsed } else { 5.0 }
}

fn detect_encoders() -> Result<Vec<String>> {
    let out = Command::new("ffmpeg")
        .args(["-hide_banner", "-encoders"]).output()
        .context("ffmpeg not found in PATH")?;
    let text = String::from_utf8_lossy(&out.stdout);
    Ok(text.lines()
        .filter(|l| l.starts_with(" V"))
        .filter_map(|l| l.split_whitespace().nth(1))
        .map(String::from).collect())
}

fn pick_best(available: &[String]) -> String {
    for c in &[
        "libsvtav1", "av1_nvenc", "hevc_vaapi", "h264_vaapi",
        "hevc_nvenc", "h264_nvenc", "libvpx-vp9", "libx265",
    ] {
        if available.contains(&c.to_string()) { return c.to_string(); }
    }
    "libx264".to_string()
}

fn vaapi_device() -> String {
    for i in 128..=129 {
        let p = format!("/dev/dri/renderD{i}");
        if std::path::Path::new(&p).exists() { return p; }
    }
    "/dev/dri/renderD128".into()
}

fn setup_vaapi_env() {
    let mut paths = vec!["/run/opengl-driver/lib/dri".to_string()];
    if let Ok(iter) = std::fs::read_dir("/nix/store") {
        for entry in iter.flatten() {
            let name = entry.path().to_string_lossy().to_string();
            if name.contains("intel-media-driver") {
                let candidate = format!("{name}/lib/dri");
                if std::path::Path::new(&candidate).exists() {
                    paths.push(candidate);
                }
                break;
            }
        }
    }
    std::env::set_var("LIBVA_DRIVERS_PATH", paths.join(":"));
}
