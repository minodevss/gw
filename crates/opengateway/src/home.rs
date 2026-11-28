use std::fs::DirBuilder;
use std::os::unix::fs::DirBuilderExt as _;
use std::path::{Path, PathBuf};

pub(crate) fn resolve(explicit: Option<PathBuf>) -> PathBuf {
    explicit
        .or_else(|| std::env::var_os("OPENGATEWAY_HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".opengateway")))
        .unwrap_or_else(|| PathBuf::from(".opengateway"))
}

// Only a directory OpenGateway creates gets 0700. An existing one, such as a volume mounted into a container,
// keeps the permissions its owner gave it: changing them needs ownership, which a non-root process
// on a root-owned mount does not have.
pub(crate) fn database(home: &Path) -> std::io::Result<PathBuf> {
    DirBuilder::new().recursive(true).mode(0o700).create(home)?;
    Ok(home.join("opengateway.db"))
}

pub(crate) fn overrides(home: &Path) -> PathBuf {
    home.join("catalog.override.toml")
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

    #[test]
    fn creates_a_private_directory_and_leaves_an_existing_one_alone() {
        let root = std::env::temp_dir().join(format!("opengateway-home-{}", std::process::id()));
        let created = root.join("new");
        database(&created).unwrap();
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&created), 0o700);

        let existing = root.join("existing");
        std::fs::create_dir_all(&existing).unwrap();
        std::fs::set_permissions(&existing, std::fs::Permissions::from_mode(0o750)).unwrap();
        database(&existing).unwrap();
        assert_eq!(mode(&existing), 0o750);
        std::fs::remove_dir_all(root).unwrap();
    }
}
