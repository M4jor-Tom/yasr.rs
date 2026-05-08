use anyhow::{Context, Result};
use std::process::Command;

pub fn detect_encoders() -> Result<Vec<String>> {
    let out = Command::new("ffmpeg")
        .args(["-hide_banner", "-encoders"]).output()
        .context("ffmpeg not found in PATH")?;
    let text = String::from_utf8_lossy(&out.stdout);
    Ok(text.lines()
        .filter(|l| l.starts_with(" V"))
        .filter_map(|l| l.split_whitespace().nth(1))
        .map(String::from).collect())
}

pub fn pick_best(available: &[String]) -> String {
    for c in &[
        "libsvtav1", "av1_nvenc", "hevc_vaapi", "h264_vaapi",
        "hevc_nvenc", "h264_nvenc", "libvpx-vp9", "libx265",
    ] {
        if available.contains(&c.to_string()) { return c.to_string(); }
    }
    "libx264".to_string()
}

pub fn list_codecs(encoders: &[String]) {
    println!("Available video encoders:");
    for c in encoders { println!("  {c}"); }
}

pub fn container_for(output: &str) -> &str {
    let ext = std::path::Path::new(output)
        .extension().and_then(|e| e.to_str()).unwrap_or("webm");
    if ext == "webm" { "webm" } else { "mp4" }
}

pub fn build_args(
    width: u32, height: u32, fps: u32, codec: &str, bitrate: Option<&str>, output: &str,
) -> Vec<String> {
    let mut args = vec![
        "-y".into(), "-f".into(), "rawvideo".into(),
        "-pix_fmt".into(), "bgra".into(),
        "-s".into(), format!("{width}x{height}"),
        "-r".into(), fps.to_string(),
        "-i".into(), "-".into(),
    ];

    match codec {
        c if c.ends_with("_vaapi") => {
            args.extend_from_slice(&[
                "-vaapi_device".into(), crate::vaapi::device_path(),
                "-vf".into(), "format=nv12,hwupload".into(),
                "-c:v".into(), c.to_string(),
            ]);
        }
        c if c.ends_with("_nvenc") => {
            args.extend_from_slice(&["-c:v".into(), c.to_string(), "-pix_fmt".into(), "yuv420p".into()]);
        }
        "libsvtav1" => {
            args.extend_from_slice(&[
                "-c:v".into(), "libsvtav1".into(), "-preset".into(), "10".into(),
                "-svtav1-params".into(), "tune=0:enable-overlays=1".into(),
            ]);
        }
        "libvpx-vp9" => {
            args.extend_from_slice(&[
                "-c:v".into(), "libvpx-vp9".into(), "-cpu-used".into(), "5".into(),
                "-deadline".into(), "realtime".into(), "-row-mt".into(), "1".into(),
            ]);
        }
        "libx264" => {
            args.extend_from_slice(&[
                "-c:v".into(), "libx264".into(), "-preset".into(), "ultrafast".into(),
                "-tune".into(), "zerolatency".into(),
            ]);
        }
        c => { args.extend_from_slice(&["-c:v".into(), c.to_string()]); }
    }

    if let Some(br) = bitrate {
        args.extend_from_slice(&["-b:v".into(), br.to_string()]);
    } else {
        let br = match codec {
            c if c.contains("vp9") => Some("2M"),
            c if c.contains("av1") => Some("1.5M"),
            c if c.contains("nvenc") => Some("4M"),
            c if c.contains("h264") && !c.contains("vaapi") => Some("4M"),
            _ => None,
        };
        if let Some(b) = br {
            args.extend_from_slice(&["-b:v".into(), b.to_string()]);
        }
    }

    let fmt = container_for(output);
    args.extend_from_slice(&["-f".into(), fmt.into(), output.into()]);
    args
}
