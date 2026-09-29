//! Path jail: resolve and validate paths inside the project directory.
//! Rejects `..` traversal, symlinks escaping the jail, and empty paths.

use std::path::{Component, Path, PathBuf};

use crate::error::EngineError;

/// Resolve `rel` inside `project_dir`, refusing traversal outside it.
/// Symlinks are resolved and re-checked (canonicalize on the deepest
/// existing ancestor + suffix).
///
/// # Errors
/// [`EngineError::UnsafePath`] when the path escapes the jail.
pub fn resolve_in_project(project_dir: &Path, rel: &str) -> Result<PathBuf, EngineError> {
    if rel.trim().is_empty() {
        return Err(EngineError::UnsafePath("empty path".into()));
    }
    let p = Path::new(rel);
    if p.is_absolute() {
        // Absolute paths are allowed only if they resolve inside the jail.
        return check_canonical(project_dir, p);
    }
    if p.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(EngineError::UnsafePath(format!(
            "`..` component rejected: {rel}"
        )));
    }
    let joined = project_dir.join(p);
    check_canonical(project_dir, &joined)
}

fn check_canonical(project_dir: &Path, candidate: &Path) -> Result<PathBuf, EngineError> {
    let root = project_dir
        .canonicalize()
        .map_err(|e| EngineError::UnsafePath(format!("project dir: {e}")))?;
    // Canonicalize the deepest existing ancestor to catch symlink escapes.
    let mut anc = candidate.to_path_buf();
    let mut suffix: Vec<std::ffi::OsString> = Vec::new();
    loop {
        match anc.canonicalize() {
            Ok(real) => {
                let mut resolved = real;
                for part in suffix.iter().rev() {
                    resolved.push(part);
                }
                if resolved.starts_with(&root) {
                    return Ok(resolved);
                }
                return Err(EngineError::UnsafePath(format!(
                    "{candidate:?} resolves outside project jail"
                )));
            }
            Err(_) => {
                let last = anc
                    .file_name()
                    .map(|f| f.to_os_string())
                    .ok_or_else(|| EngineError::UnsafePath("bad path".into()))?;
                suffix.push(last);
                if !anc.pop() {
                    return Err(EngineError::UnsafePath("path root unreachable".into()));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn resolves_relative_inside_jail() {
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("media");
        fs::create_dir_all(&media).unwrap();
        fs::write(media.join("a.mp4"), b"x").unwrap();
        let got = resolve_in_project(dir.path(), "media/a.mp4").unwrap();
        assert!(got.ends_with("media/a.mp4"));
    }

    #[test]
    fn rejects_parent_traversal() {
        let dir = tempfile::tempdir().unwrap();
        assert!(resolve_in_project(dir.path(), "../secret.mp4").is_err());
        assert!(resolve_in_project(dir.path(), "media/../../secret.mp4").is_err());
        assert!(resolve_in_project(dir.path(), "").is_err());
    }

    #[test]
    fn rejects_symlink_escape() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("t.mp4"), b"x").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path().join("t.mp4"), dir.path().join("link.mp4"))
            .unwrap();
        let r = resolve_in_project(dir.path(), "link.mp4");
        #[cfg(unix)]
        assert!(r.is_err(), "symlink escape must be rejected");
    }

    #[test]
    fn rejects_absolute_outside() {
        let dir = tempfile::tempdir().unwrap();
        assert!(resolve_in_project(dir.path(), "/etc/passwd").is_err());
    }
}
