use anyhow::{Context, Result};
use clap::Parser;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Terminal;
use std::io::{stdout, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use xcap::Monitor;
use yasr::{capture, encode, vaapi};

#[derive(Parser)]
#[command(version, about = "Yet Another Screen Recorder — TUI")]
struct Args {
    #[arg(default_value = "output.webm")]
    output: String,

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
}

struct Recorder {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    start: Instant,
    frames: u64,
    target_fps: u32,
    interval: Duration,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let monitors = Monitor::all().context("Failed to enumerate monitors")?;
    if monitors.is_empty() {
        anyhow::bail!("No monitors found");
    }

    let available = encode::detect_encoders()?;
    let codec = args
        .codec
        .clone()
        .unwrap_or_else(|| encode::pick_best(&available));

    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;

    let res = run_tui(&mut terminal, &monitors, &args, &codec);

    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    if let Err(ref e) = res {
        eprintln!("Error: {e:#}");
    }

    res
}

fn run_tui(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    monitors: &[Monitor],
    args: &Args,
    codec: &str,
) -> Result<()> {
    let mut selected = 0;
    let mut recorder: Option<Recorder> = None;
    let mut error_msg: Option<String> = None;

    let running = Arc::new(AtomicBool::new(true));
    let sig = running.clone();
    ctrlc::set_handler(move || sig.store(false, Ordering::SeqCst))?;

    'main: loop {
        terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(7),
                    Constraint::Min(6),
                    Constraint::Length(3),
                ])
                .split(f.area());

            let top = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(chunks[0]);

            // ── Info panel ──
            let mon_name = monitors[selected]
                .name()
                .unwrap_or_else(|_| format!("monitor {}", selected));
            let mw = monitors[selected].width().unwrap_or(0);
            let mh = monitors[selected].height().unwrap_or(0);

            let info_lines = vec![
                Line::from(Span::styled(
                    "yasr — Screen Recorder",
                    Style::default().add_modifier(Modifier::BOLD),
                )),
                Line::from(format!("Monitor [{selected}]: {mon_name} ({mw}x{mh})")),
                Line::from(format!("Encoder: {codec}")),
                Line::from(format!("Output: {}", args.output)),
                Line::from(format!("Target FPS: {}", args.fps)),
            ];
            f.render_widget(
                Paragraph::new(Text::from(info_lines))
                    .block(Block::default().borders(Borders::ALL).title(" Info ")),
                top[0],
            );

            // ── Status panel ──
            let mut status_lines: Vec<Line> = vec![];
            if let Some(ref rec) = recorder {
                let elapsed = rec.start.elapsed();
                let secs = elapsed.as_secs();
                let actual_fps = if secs > 0 {
                    rec.frames as f64 / elapsed.as_secs_f64()
                } else {
                    0.0
                };
                status_lines.push(Line::from(Span::styled(
                    "● RECORDING",
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                )));
                status_lines.push(Line::from(format!(
                    "Duration: {:02}:{:02}:{:02}",
                    secs / 3600,
                    secs / 60 % 60,
                    secs % 60,
                )));
                status_lines.push(Line::from(format!("Frames: {}", rec.frames)));
                status_lines.push(Line::from(format!(
                    "FPS: {actual_fps:.1} (target: {})",
                    rec.target_fps
                )));
            } else {
                status_lines.push(Line::from("Ready"));
                status_lines.push(Line::from(""));
                status_lines.push(Line::from("Press SPACE to start recording"));
            }
            if let Some(ref err) = error_msg {
                status_lines.push(Line::from(""));
                status_lines.push(Line::from(Span::styled(
                    err,
                    Style::default().fg(Color::Yellow),
                )));
            }
            f.render_widget(
                Paragraph::new(Text::from(status_lines))
                    .block(Block::default().borders(Borders::ALL).title(" Status ")),
                top[1],
            );

            // ── Controls ──
            let controls = if recorder.is_some() {
                " [Space] Stop   [q] Quit"
            } else if monitors.len() > 1 {
                " [Space] Start   [←/→] Monitor   [q] Quit"
            } else {
                " [Space] Start   [q] Quit"
            };
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    controls,
                    Style::default().fg(Color::Cyan),
                )))
                .block(Block::default().borders(Borders::ALL).title(" Controls ")),
                chunks[2],
            );
        })?;

        if !running.load(Ordering::SeqCst) {
            break;
        }

        // ── Event poll (16ms ≈ 60 Hz UI) ──
        let has_event = event::poll(Duration::from_millis(16))?;

        if has_event {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') => break,
                        KeyCode::Char(' ') => {
                            if recorder.is_some() {
                                // Stop recording
                                stop_recording(&mut recorder)?;
                            } else {
                                // Start recording
                                match start_recording(monitors, selected, args, codec) {
                                    Ok(r) => recorder = Some(r),
                                    Err(e) => error_msg = Some(format!("{e:#}")),
                                }
                            }
                        }
                        KeyCode::Left | KeyCode::Up if recorder.is_none() => {
                            selected = selected.saturating_sub(1);
                        }
                        KeyCode::Right | KeyCode::Down if recorder.is_none() => {
                            selected = (selected + 1).min(monitors.len() - 1);
                        }
                        _ => {}
                    }
                }
            }
        }

        // ── Capture frame if recording ──
        if recorder.is_some() {
            let should_stop;

            // Narrow scope so the ref-mut borrow of recorder drops before our take()
            {
                let rec = recorder.as_mut().unwrap();
                let frame_start = Instant::now();

                match monitors[selected].capture_image() {
                    Ok(img) => match rec.stdin.write_all(&img) {
                        Ok(()) => {
                            rec.frames += 1;
                            should_stop = false;
                        }
                        Err(e) => {
                            error_msg = Some(format!("ffmpeg write error: {e}"));
                            should_stop = true;
                        }
                    },
                    Err(e) => {
                        error_msg = Some(format!("capture error: {e}"));
                        should_stop = false;
                    }
                }

                if !should_stop {
                    if let Some(sleep) = rec.interval.checked_sub(frame_start.elapsed()) {
                        std::thread::sleep(sleep);
                    }
                }
            }

            if should_stop {
                stop_recording(&mut recorder)?;
                continue 'main;
            }
        }
    }

    // Clean up on exit
    if recorder.is_some() {
        stop_recording(&mut recorder)?;
    }

    Ok(())
}

fn start_recording(
    monitors: &[Monitor],
    selected: usize,
    args: &Args,
    codec: &str,
) -> Result<Recorder> {
    let mon = &monitors[selected];
    let width = mon.width()?;
    let height = mon.height()?;

    let target_fps = if args.fps == "auto" {
        let cap_fps = capture::probe_fps(mon);
        (cap_fps as u32).clamp(1, 30)
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
        codec,
        args.bitrate.as_deref(),
        &args.output,
    );

    let mut child = Command::new("ffmpeg")
        .args(&ffmpeg_args)
        .stdin(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .context("failed to spawn ffmpeg")?;
    let stdin = child.stdin.take().context("no stdin on ffmpeg")?;

    Ok(Recorder {
        child,
        stdin,
        start: Instant::now(),
        frames: 0,
        target_fps,
        interval: Duration::from_secs_f64(1.0 / target_fps as f64),
    })
}

fn stop_recording(recorder: &mut Option<Recorder>) -> Result<()> {
    if let Some(mut rec) = recorder.take() {
        drop(rec.stdin);
        let _ = rec.child.wait();
    }
    Ok(())
}
