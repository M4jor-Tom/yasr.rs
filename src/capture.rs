use anyhow::Result;
use std::time::Instant;
use xcap::Monitor;

pub fn list_monitors(monitors: &[Monitor]) -> Result<()> {
    for (i, m) in monitors.iter().enumerate() {
        let primary = m.is_primary()?;
        println!(
            "[{i}] {} ({}x{} @ {})",
            m.name()?,
            m.width()?,
            m.height()?,
            if primary { "primary" } else { "secondary" },
        );
    }
    Ok(())
}

pub fn probe_fps(mon: &Monitor) -> f64 {
    let n = 4;
    let mut elapsed = 0.0;
    for i in 0..n {
        let start = Instant::now();
        if mon.capture_image().is_ok() {
            let t = start.elapsed().as_secs_f64();
            if i > 0 {
                elapsed += t;
            }
        }
    }
    if elapsed > 0.0 {
        (n - 1) as f64 / elapsed
    } else {
        5.0
    }
}
