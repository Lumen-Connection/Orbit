//! Canonicalize a path and reject anything that escapes the project root.

use std::path::{Component, Path, PathBuf};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SecurityError {
    #[error("path escapes the project root")]
    EscapesRoot,
    #[error("could not resolve path: {0}")]
    Io(String),
}

/// Join `relative` onto `root`, resolve symlinks, and require the result to
/// stay under the canonical root.
pub fn resolve_within_root(root: &Path, relative: &Path) -> Result<PathBuf, SecurityError> {
    let canonical_root = root
        .canonicalize()
        .map_err(|e| SecurityError::Io(e.to_string()))?;

    let candidate = if relative.is_absolute() {
        relative.to_path_buf()
    } else {
        canonical_root.join(relative)
    };

    let resolved = if candidate.exists() {
        candidate
            .canonicalize()
            .map_err(|e| SecurityError::Io(e.to_string()))?
    } else {
        resolve_missing(&candidate)?
    };

    if !is_under(&resolved, &canonical_root) {
        return Err(SecurityError::EscapesRoot);
    }
    Ok(resolved)
}

/// Walk up to the first existing ancestor, canonicalize it (so a symlink
/// escape is visible), then re-append the missing tail.
fn resolve_missing(candidate: &Path) -> Result<PathBuf, SecurityError> {
    let mut current = candidate.to_path_buf();
    let mut missing = Vec::new();
    while !current.exists() {
        match current.file_name() {
            Some(name) => missing.push(name.to_os_string()),
            None => return Err(SecurityError::EscapesRoot),
        }
        match current.parent() {
            Some(parent) => current = parent.to_path_buf(),
            None => return Err(SecurityError::EscapesRoot),
        }
    }
    let mut resolved = current
        .canonicalize()
        .map_err(|e| SecurityError::Io(e.to_string()))?;
    for name in missing.into_iter().rev() {
        resolved.push(name);
    }
    Ok(resolved)
}

fn is_under(path: &Path, root: &Path) -> bool {
    let path = strip_verbatim(path);
    let root = strip_verbatim(root);
    if path == root {
        return true;
    }
    let path_comps: Vec<Component<'_>> = path.components().collect();
    let root_comps: Vec<Component<'_>> = root.components().collect();
    path_comps.starts_with(&root_comps)
}

fn strip_verbatim(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let raw = path.as_os_str().to_string_lossy();
        if let Some(rest) = raw.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
    }
    path.to_path_buf()
}

/// Secrets and VCS metadata that must never enter an automatic digest.
pub fn is_sensitive(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    if name == ".env" || name.starts_with(".env.") || name == "id_rsa" {
        return true;
    }
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        let ext = ext.to_ascii_lowercase();
        if matches!(ext.as_str(), "pem" | "key" | "p12") {
            return true;
        }
    }
    let parts: Vec<&str> = path
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();
    parts.windows(2).any(|w| w[0] == ".git" && w[1] == "config")
}

#[cfg(test)]
mod tests {
    use super::{SecurityError, is_sensitive, resolve_within_root};
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    fn fixture() -> (TempDir, std::path::PathBuf) {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("proj");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src").join("main.rs"), "fn main() {}").unwrap();
        (tmp, root)
    }

    #[test]
    fn accepts_a_file_inside_the_root() {
        let (_tmp, root) = fixture();
        let resolved = resolve_within_root(&root, Path::new("src/main.rs")).unwrap();
        assert!(resolved.ends_with("main.rs"));
    }

    #[test]
    fn rejects_parent_directory_escape() {
        let (_tmp, root) = fixture();
        let err = resolve_within_root(&root, Path::new("../../etc/passwd")).unwrap_err();
        assert_eq!(err, SecurityError::EscapesRoot);
    }

    #[test]
    fn rejects_absolute_path_outside_the_root() {
        let (_tmp, root) = fixture();
        #[cfg(windows)]
        let outside = Path::new(r"C:\Windows\System32\drivers\etc\hosts");
        #[cfg(not(windows))]
        let outside = Path::new("/etc/passwd");
        if !outside.exists() {
            return;
        }
        let err = resolve_within_root(&root, outside).unwrap_err();
        assert_eq!(err, SecurityError::EscapesRoot);
    }

    #[test]
    fn rejects_symlink_inside_project_pointing_out() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("proj");
        fs::create_dir_all(&root).unwrap();
        let secret = tmp.path().join("secret.txt");
        fs::write(&secret, "classified").unwrap();
        let link = root.join("escape");

        let made = make_symlink(&secret, &link);
        if made.is_err() {
            // Windows may refuse symlink creation without Developer Mode.
            return;
        }
        let err = resolve_within_root(&root, Path::new("escape")).unwrap_err();
        assert_eq!(err, SecurityError::EscapesRoot);
    }

    #[test]
    fn detects_sensitive_names() {
        assert!(is_sensitive(Path::new(".env")));
        assert!(is_sensitive(Path::new(".env.local")));
        assert!(is_sensitive(Path::new("certs/server.pem")));
        assert!(is_sensitive(Path::new("id_rsa")));
        assert!(is_sensitive(Path::new(".git/config")));
        assert!(is_sensitive(Path::new("src/keystore.p12")));
        assert!(is_sensitive(Path::new("tls/app.key")));
        assert!(!is_sensitive(Path::new("src/main.rs")));
    }

    fn make_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link)
        }
        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_file(target, link)
        }
    }
}
