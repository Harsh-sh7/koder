//! Small shared helpers with no domain dependencies.

/// Formats a byte count for display (e.g. `1.4 MB`).
///
/// @param bytes raw byte count
/// @returns human-readable string with one decimal place for KiB and above
pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Shortens a path for display by replacing the home directory with `~`.
///
/// @param path absolute path to display
/// @returns display string
pub fn display_path(path: &std::path::Path) -> String {
    if let Some(home) = directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf()) {
        if let Ok(stripped) = path.strip_prefix(&home) {
            return format!("~/{}", stripped.display());
        }
    }
    path.display().to_string()
}
