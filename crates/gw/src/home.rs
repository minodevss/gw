use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

pub(crate) fn resolve(explicit: Option<PathBuf>) -> PathBuf {
    explicit
        .or_else(|| std::env::var_os("GW_HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".gw")))
        .unwrap_or_else(|| PathBuf::from(".gw"))
}

pub(crate) fn database(home: &Path) -> std::io::Result<PathBuf> {
    fs::create_dir_all(home)?;
    fs::set_permissions(home, fs::Permissions::from_mode(0o700))?;
    Ok(home.join("gw.db"))
}

pub(crate) fn overrides(home: &Path) -> PathBuf {
    home.join("catalog.override.toml")
}
