use anyhow::{Context, Result};
use clap::Parser;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Terminal;
use std::collections::VecDeque;
use std::io::{stdout, BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
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

    #[arg(short, long, help = "Show verbose ffmpeg output")]
    verbose: bool,
}

#[derive(PartialEq, Eq)]
enum LogLevel {
    Info,
    Warning,
    Error,
}

fn classify(text: &str) -> LogLevel {
    let lower = text.to_lowercase();
    if lower.contains("error") {
        LogLevel::Error
    } else if lower.contains("warning") {
        LogLevel::Warning
    } else {
        LogLevel::Info
    }
}

struct Recorder {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    rx: mpsc::Receiver<String>,
    stderr_thread: thread::JoinHandle<()>,
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

fn centered_rect(pct_x: u16, pct_y: u16, r: Rect) -> Rect {
    let vert = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - pct_y) / 2),
            Constraint::Percentage(pct_y),
            Constraint::Percentage((100 - pct_y) / 2),
        ])
        .split(r);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - pct_x) / 2),
            Constraint::Percentage(pct_x),
            Constraint::Percentage((100 - pct_x) / 2),
        ])
        .split(vert[1])[1]
}

fn level_color(level: &LogLevel) -> Color {
    match level {
        LogLevel::Error => Color::Red,
        LogLevel::Warning => Color::Yellow,
        LogLevel::Info => Color::White,
    }
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
    let mut log_buffer: VecDeque<(LogLevel, String)> = VecDeque::new();
    let mut show_log = false;
    let mut log_scroll: usize = 0;

    let running = Arc::new(AtomicBool::new(true));
    let sig = running.clone();
    ctrlc::set_handler(move || sig.store(false, Ordering::SeqCst))?;

    'main: loop {
        // Drain stderr lines from ffmpeg
        if let Some(ref rec) = recorder {
            while let Ok(line) = rec.rx.try_recv() {
                let level = classify(&line);
                log_buffer.push_back((level, line));
                if log_buffer.len() > 50 {
                    log_buffer.pop_front();
                }
            }
        }

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

            // ── Controls bar ──
            let mut ctl_parts: Vec<Span> = vec![];

            if !log_buffer.is_empty() {
                let max_level = log_buffer
                    .iter()
                    .max_by_key(|(lvl, _)| match lvl {
                        LogLevel::Error => 2,
                        LogLevel::Warning => 1,
                        LogLevel::Info => 0,
                    })
                    .map(|(lvl, _)| lvl)
                    .unwrap();
                let color = match max_level {
                    LogLevel::Error => Color::Red,
                    LogLevel::Warning => Color::Yellow,
                    LogLevel::Info => Color::Cyan,
                };
                let prefix = if *max_level == LogLevel::Error {
                    " ⚠"
                } else {
                    " "
                };
                ctl_parts.push(Span::styled(
                    format!("{prefix}[e] log ({})", log_buffer.len()),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ));
                ctl_parts.push(Span::raw("  "));
            }

            if recorder.is_some() {
                ctl_parts.push(Span::styled(
                    "[Space] Stop",
                    Style::default().fg(Color::Cyan),
                ));
                ctl_parts.push(Span::raw("  "));
                ctl_parts.push(Span::styled("[q] Quit", Style::default().fg(Color::Cyan)));
            } else if monitors.len() > 1 {
                ctl_parts.push(Span::styled(
                    "[Space] Start",
                    Style::default().fg(Color::Cyan),
                ));
                ctl_parts.push(Span::raw("  "));
                ctl_parts.push(Span::styled(
                    "[←/→] Monitor",
                    Style::default().fg(Color::Cyan),
                ));
                ctl_parts.push(Span::raw("  "));
                ctl_parts.push(Span::styled("[q] Quit", Style::default().fg(Color::Cyan)));
            } else {
                ctl_parts.push(Span::styled(
                    "[Space] Start",
                    Style::default().fg(Color::Cyan),
                ));
                ctl_parts.push(Span::raw("  "));
                ctl_parts.push(Span::styled("[q] Quit", Style::default().fg(Color::Cyan)));
            }

            f.render_widget(
                Paragraph::new(Line::from(ctl_parts))
                    .block(Block::default().borders(Borders::ALL).title(" Controls ")),
                chunks[2],
            );

            // ── Log popup ──
            if show_log && !log_buffer.is_empty() {
                let area = centered_rect(70, 60, f.area());
                f.render_widget(Clear, area);

                let max_visible = (area.height as usize).saturating_sub(2);
                let total = log_buffer.len();
                if log_scroll + max_visible > total {
                    log_scroll = total.saturating_sub(max_visible);
                }

                let lines: Vec<Line> = log_buffer
                    .iter()
                    .skip(log_scroll)
                    .take(max_visible)
                    .map(|(lvl, text)| {
                        let color = level_color(lvl);
                        Line::from(Span::styled(text, Style::default().fg(color)))
                    })
                    .collect();

                f.render_widget(
                    Paragraph::new(Text::from(lines))
                        .block(Block::default().borders(Borders::ALL).title(" FFmpeg log ")),
                    area,
                );
            }
        })?;

        if !running.load(Ordering::SeqCst) {
            break;
        }

        // ── Event poll (16ms ≈ 60 Hz UI) ──
        let has_event = event::poll(Duration::from_millis(16))?;

        if has_event {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    let was_log_open = show_log;
                    match key.code {
                        KeyCode::Char('q') => break,
                        KeyCode::Char('e') => {
                            if log_buffer.is_empty() {
                                show_log = false;
                            } else {
                                show_log = !show_log;
                            }
                        }
                        KeyCode::Esc => {
                            show_log = false;
                        }
                        KeyCode::Up if show_log => {
                            log_scroll = log_scroll.saturating_sub(1);
                        }
                        KeyCode::Down if show_log => {
                            log_scroll = log_scroll.saturating_add(1);
                        }
                        KeyCode::Char(' ') if !show_log => {
                            if recorder.is_some() {
                                stop_recording(&mut recorder)?;
                            } else {
                                match start_recording(monitors, selected, args, codec) {
                                    Ok(r) => recorder = Some(r),
                                    Err(e) => error_msg = Some(format!("{e:#}")),
                                }
                            }
                        }
                        KeyCode::Left | KeyCode::Up if recorder.is_none() && !was_log_open => {
                            selected = selected.saturating_sub(1);
                        }
                        KeyCode::Right | KeyCode::Down if recorder.is_none() && !was_log_open => {
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
        args.verbose,
    );

    let mut child = Command::new("ffmpeg")
        .args(&ffmpeg_args)
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to spawn ffmpeg")?;
    let stdin = child.stdin.take().context("no stdin on ffmpeg")?;
    let stderr = child.stderr.take().context("no stderr on ffmpeg")?;

    let (tx, rx) = mpsc::channel();
    let stderr_thread = thread::spawn(move || {
        let reader = BufReader::new(stderr);
        for line in reader.lines() {
            match line {
                Ok(l) => {
                    if tx.send(l).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    Ok(Recorder {
        child,
        stdin,
        rx,
        stderr_thread,
        start: Instant::now(),
        frames: 0,
        target_fps,
        interval: Duration::from_secs_f64(1.0 / target_fps as f64),
    })
}

fn stop_recording(recorder: &mut Option<Recorder>) -> Result<()> {
    if let Some(mut rec) = recorder.take() {
        drop(rec.stdin);
        let _ = rec.child.wait()?;
        let _ = rec.stderr_thread.join();
    }
    Ok(())
}
