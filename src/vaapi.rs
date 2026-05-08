use std::path::Path;

pub fn device_path() -> String {
    for i in 128..=129 {
        let p = format!("/dev/dri/renderD{i}");
        if Path::new(&p).exists() {
            return p;
        }
    }
    "/dev/dri/renderD128".into()
}

pub fn setup_env() {
    let mut paths = vec!["/run/opengl-driver/lib/dri".to_string()];
    if let Ok(iter) = std::fs::read_dir("/nix/store") {
        for entry in iter.flatten() {
            let name = entry.path().to_string_lossy().to_string();
            if name.contains("intel-media-driver") {
                let candidate = format!("{name}/lib/dri");
                if Path::new(&candidate).exists() {
                    paths.push(candidate);
                }
                break;
            }
        }
    }
    std::env::set_var("LIBVA_DRIVERS_PATH", paths.join(":"));
}
