//! `.mycut` persistence: versioned JSON, atomic writes (temp + fsync +
//! rename), rolling backups, crash recovery, and forward migrations.
//!
//! Sources are stored project-relative; content hashes support relink.

pub mod secret;

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use mycut_core::{CoreError, History, Project, SCHEMA_VERSION};
use serde::{Deserialize, Serialize};

/// On-disk document. `schema_version` is the *document* version (may differ
/// from `project.schema_version` after migration).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectDocument {
    pub schema_version: String,
    pub project: Project,
    pub history: History,
    pub saved_at_unix_ms: i64,
    pub app_version: String,
}

const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
/// Rolling backups kept per project file.
pub const BACKUP_COUNT: usize = 5;
/// History tail serialized with the project (restart-surviving undo).
const MAX_SERIALIZED_HISTORY: usize = 200;

/// Errors specific to persistence.
#[derive(Debug, Error)]
pub enum ProjectIoError {
    #[error("project file version {found} is newer than this app supports ({supported})")]
    NewerVersion { found: String, supported: String },
    #[error("project file is corrupt: {0}")]
    Corrupt(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Serde(#[from] serde_json::Error),
    #[error(transparent)]
    Core(#[from] CoreError),
}

use thiserror::Error;

/// Canonicalize a media path into a project-relative path when possible.
/// Absolute paths outside the project dir keep an absolute form (relinkable
/// by hash), and are flagged by the UI as external.
#[must_use]
pub fn to_project_relative(media_path: &Path, project_dir: &Path) -> String {
    match media_path.canonicalize() {
        Ok(canon) => match canon.strip_prefix(
            project_dir
                .canonicalize()
                .unwrap_or_else(|_| project_dir.to_path_buf()),
        ) {
            Ok(rel) => rel.to_string_lossy().into_owned(),
            Err(_) => canon.to_string_lossy().into_owned(),
        },
        Err(_) => media_path.to_string_lossy().into_owned(),
    }
}

/// Resolve a stored source path against the project dir; verifies existence.
#[must_use]
pub fn resolve_source_path(rel_path: &str, project_dir: &Path) -> Option<PathBuf> {
    let p = PathBuf::from(rel_path);
    if p.is_absolute() {
        p.exists().then_some(p)
    } else {
        let full = project_dir.join(p);
        full.exists().then_some(full)
    }
}

/// Write bytes atomically: temp file in same dir -> fsync -> rename -> fsync dir.
///
/// # Errors
/// [`ProjectIoError::Io`] on filesystem failures.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), ProjectIoError> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(dir)?;
    let tmp = tempfile::NamedTempFile::new_in(dir)?;
    {
        let mut f = fs::File::create(tmp.path())?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    // Persist over the target atomically (same filesystem).
    tmp.persist(path).map_err(|e| ProjectIoError::Io(e.error))?;
    // Best-effort directory fsync so the rename is durable.
    if let Ok(d) = fs::File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

fn backup_path(path: &Path, n: usize) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(format!(".bak.{n}"));
    PathBuf::from(s)
}

/// Rotate rolling backups: bak.4 <- bak.3 ... bak.1 <- current (if exists).
fn rotate_backups(path: &Path) {
    for n in (1..BACKUP_COUNT).rev() {
        let from = backup_path(path, n);
        let to = backup_path(path, n + 1);
        if from.exists() {
            let _ = fs::rename(&from, &to);
        }
    }
    if path.exists() {
        let _ = fs::copy(path, backup_path(path, 1));
    }
}

/// Save project + bounded history atomically, with rolling backups.
///
/// # Errors
/// [`ProjectIoError`] on serialization/filesystem failure.
pub fn save_project(
    project: &Project,
    history: &History,
    path: &Path,
) -> Result<(), ProjectIoError> {
    rotate_backups(path);
    let mut hist = history.clone();
    hist.undo_stack.truncate(MAX_SERIALIZED_HISTORY);
    hist.redo_stack.truncate(MAX_SERIALIZED_HISTORY);
    let doc = ProjectDocument {
        schema_version: SCHEMA_VERSION.to_string(),
        project: project.clone(),
        history: hist,
        saved_at_unix_ms: mycut_core::now_unix_ms(),
        app_version: APP_VERSION.to_string(),
    };
    let bytes = serde_json::to_vec_pretty(&doc)?;
    atomic_write(path, &bytes)
}

/// Load a project document, migrating forward when needed.
///
/// # Errors
/// [`ProjectIoError::NewerVersion`] for unknown future versions,
/// [`ProjectIoError::Corrupt`] for unparseable content.
pub fn load_project(path: &Path) -> Result<ProjectDocument, ProjectIoError> {
    let bytes = fs::read(path)?;
    let doc: ProjectDocument = serde_json::from_slice(&bytes)
        .map_err(|e| ProjectIoError::Corrupt(format!("{path:?}: {e}")))?;
    migrate(doc)
}

/// Attempt main file, then backups (newest first). Returns the document and
/// which file it came from — used for the "Recovered project" flow.
pub fn load_with_recovery(path: &Path) -> Option<(ProjectDocument, PathBuf)> {
    if let Ok(doc) = load_project(path) {
        return Some((doc, path.to_path_buf()));
    }
    (1..=BACKUP_COUNT)
        .map(|n| backup_path(path, n))
        .filter(|p| p.exists())
        .find_map(|p| load_project(&p).ok().map(|doc| (doc, p)))
}

/// Forward migration chain. Currently v1.0 only; future versions append
/// `fn v1_0_to_v1_1(...) -> ...` steps here.
fn migrate(doc: ProjectDocument) -> Result<ProjectDocument, ProjectIoError> {
    match doc.schema_version.as_str() {
        SCHEMA_VERSION => Ok(doc),
        other => Err(ProjectIoError::NewerVersion {
            found: other.to_string(),
            supported: SCHEMA_VERSION.to_string(),
        }),
    }
}

/// Content hash for a media file (BLAKE-style via sha2 over size + head/tail
/// 1 MiB windows): fast, stable, collision-safe for relink purposes.
///
/// # Errors
/// [`ProjectIoError::Io`] on read failure.
pub fn content_hash(path: &Path) -> Result<String, ProjectIoError> {
    use sha2::{Digest, Sha256};
    let meta = fs::metadata(path)?;
    let mut hasher = Sha256::new();
    hasher.update(format!("size:{}", meta.len()));
    let mut f = fs::File::open(path)?;
    use std::io::Read;
    let window: u64 = 1024 * 1024;
    let head = {
        let mut buf = vec![0u8; window.min(meta.len()) as usize];
        f.read_exact(&mut buf)?;
        buf
    };
    hasher.update(&head);
    if meta.len() > window {
        let tail_start = meta.len() - window;
        use std::io::{Read, Seek, SeekFrom};
        f.seek(SeekFrom::Start(tail_start))?;
        let mut buf = vec![0u8; window as usize];
        f.read_exact(&mut buf)?;
        hasher.update(&buf);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Write a crash-recovery journal marker used on next launch.
pub fn write_journal(path: &Path) -> Result<(), ProjectIoError> {
    let j = path.with_extension("mycut.journal");
    atomic_write(&j, mycut_core::now_unix_ms().to_string().as_bytes())
}

pub fn clear_journal(path: &Path) {
    let j = path.with_extension("mycut.journal");
    let _ = fs::remove_file(j);
}

#[must_use]
pub fn has_journal(path: &Path) -> bool {
    path.with_extension("mycut.journal").exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mycut_core::{Command, Item, ItemKind, TrackKind};

    fn tmpdir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn sample_project() -> Project {
        let mut p = Project::new("sample");
        p.name = "sample".into();
        p
    }

    #[test]
    fn save_load_roundtrip_identical() {
        let dir = tmpdir();
        let path = dir.path().join("test.mycut");
        let p = sample_project();
        let mut h = History::new();
        let item = Item::new(
            ItemKind::Text {
                kind: mycut_core::TextKind::Title,
                text: "Hello".into(),
                position: mycut_core::Position::Center,
                scale: 1.0,
                opacity: 1.0,
            },
            0,
            2_000,
        );
        let mut p2 = p.clone();
        h.apply(
            &mut p2,
            Command::AddClip {
                track_kind: TrackKind::Text,
                item,
            },
        )
        .unwrap();
        save_project(&p2, &h, &path).unwrap();
        let doc = load_project(&path).unwrap();
        assert_eq!(doc.project.name, "sample");
        assert_eq!(doc.project.tracks[3].items.len(), 1);
        // Identical re-save/re-load is byte-stable in the model.
        let before = serde_json::to_string(&doc.project).unwrap();
        save_project(&doc.project, &doc.history, &path).unwrap();
        let doc2 = load_project(&path).unwrap();
        let after = serde_json::to_string(&doc2.project).unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn atomic_write_leaves_no_tmp_files() {
        let dir = tmpdir();
        let path = dir.path().join("x.mycut");
        atomic_write(&path, b"hello").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"hello");
        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temp files must be cleaned up");
    }

    #[test]
    fn backups_rotate_and_recover() {
        let dir = tmpdir();
        let path = dir.path().join("r.mycut");
        let p = sample_project();
        let h = History::new();
        // Two good saves: main + bak.1 both hold valid content.
        save_project(&p, &h, &path).unwrap();
        save_project(&p, &h, &path).unwrap();
        // Corrupt only the main file.
        fs::write(&path, b"{corrupt").unwrap();
        let (doc, src) = load_with_recovery(&path).expect("should recover from backup");
        assert_ne!(src, path, "recovered from a backup");
        assert_eq!(doc.project.name, "sample");
    }

    #[test]
    fn newer_version_refused() {
        let doc = ProjectDocument {
            schema_version: "99.0".into(),
            project: sample_project(),
            history: History::new(),
            saved_at_unix_ms: 0,
            app_version: APP_VERSION.into(),
        };
        assert!(matches!(
            migrate(doc),
            Err(ProjectIoError::NewerVersion { .. })
        ));
    }

    #[test]
    fn journal_lifecycle() {
        let dir = tmpdir();
        let path = dir.path().join("j.mycut");
        assert!(!has_journal(&path));
        write_journal(&path).unwrap();
        assert!(has_journal(&path));
        clear_journal(&path);
        assert!(!has_journal(&path));
    }
}
