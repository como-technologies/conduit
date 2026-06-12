//! `.conduit/` file store — atomic writes, task records, plan snapshots, cursors.
//!
//! On-disk layout:
//!
//! ```text
//! .conduit/
//! ├── tasks/<task-id>.json     TaskRecord incl. pending ActionIntents
//! ├── plans/<task-id>.md       verbatim plan snapshot (sha256 on the record)
//! ├── cursor/<forge>.json      previous RepoSnapshot per forge (the poll cursor)
//! ├── cache/<forge>.git        local bare git cache (Task 11, git.rs)
//! ├── workspaces/<task-id>-a<attempt>/   engine workspaces (disposable)
//! └── bin/                     pinned adroit (Task 10, `just init-adroit`)
//! ```
//!
//! All record/plan/cursor writes are atomic: bytes go to `<path>.tmp`, fsynced,
//! then renamed over the destination. Parent dir is also fsynced so the rename
//! is durable. A partially-written file can never be observed (spec §Crash
//! consistency).
//!
//! Action intents are persisted on the `TaskRecord` **before** the router
//! executes them. The router calls `mark_intent_done` after each action
//! succeeds.

use std::path::{Path, PathBuf};

use crate::task::TaskRecord;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("store I/O at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("corrupt record {path}: {source}")]
    Corrupt {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("no plan snapshot for task {0}")]
    MissingPlan(String),
}

pub struct Store {
    root: PathBuf, // the .conduit dir
}

impl Store {
    /// Open (and create dirs under) `<repo>/.conduit`.
    pub fn open(root: impl Into<PathBuf>) -> Result<Store, StoreError> {
        let root: PathBuf = root.into();
        for sub in &["tasks", "plans", "cursor", "cache", "workspaces", "bin"] {
            let dir = root.join(sub);
            std::fs::create_dir_all(&dir).map_err(|source| StoreError::Io {
                path: dir.clone(),
                source,
            })?;
        }
        Ok(Store { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn workspace_dir(&self, task_id: &str, attempt: u32) -> PathBuf {
        self.root
            .join("workspaces")
            .join(format!("{task_id}-a{attempt}"))
    }

    // Tasks — atomic tmp+rename+fsync (file AND parent dir).

    pub fn save_task(&self, rec: &TaskRecord) -> Result<(), StoreError> {
        let path = self.task_path(&rec.id);
        let bytes = serde_json::to_vec_pretty(rec).map_err(|source| StoreError::Corrupt {
            path: path.clone(),
            source,
        })?;
        write_atomic(&path, &bytes)
    }

    pub fn load_task(&self, id: &str) -> Result<Option<TaskRecord>, StoreError> {
        let path = self.task_path(id);
        match std::fs::read(&path) {
            Ok(bytes) => {
                let rec = serde_json::from_slice(&bytes).map_err(|source| StoreError::Corrupt {
                    path: path.clone(),
                    source,
                })?;
                Ok(Some(rec))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(StoreError::Io { path, source }),
        }
    }

    pub fn list_tasks(&self) -> Result<Vec<TaskRecord>, StoreError> {
        let dir = self.root.join("tasks");
        let mut records = Vec::new();
        for entry in std::fs::read_dir(&dir).map_err(|source| StoreError::Io {
            path: dir.clone(),
            source,
        })? {
            let entry = entry.map_err(|source| StoreError::Io {
                path: dir.clone(),
                source,
            })?;
            let path = entry.path();
            // Only process files ending in exactly ".json" (skip ".json.tmp" etc.)
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            // Extra guard: the stem must not end with ".json" (i.e. no "foo.json.json")
            // but primarily we need to exclude things like "foo.json.tmp" —
            // those have extension "tmp", already skipped above.
            let bytes = std::fs::read(&path).map_err(|source| StoreError::Io {
                path: path.clone(),
                source,
            })?;
            let rec = serde_json::from_slice(&bytes).map_err(|source| StoreError::Corrupt {
                path: path.clone(),
                source,
            })?;
            records.push(rec);
        }
        Ok(records)
    }

    /// Load-modify-save: set `pending[index].done = true`. Atomic.
    ///
    /// Caller invariant (router): `index` must come from the SAME load that
    /// produced the intents being executed — never from a previous tick's
    /// record. The daemon is single-threaded, so load-modify-save is safe
    /// within one tick.
    pub fn mark_intent_done(&self, task_id: &str, index: usize) -> Result<(), StoreError> {
        let mut rec = self.load_task(task_id)?.ok_or_else(|| StoreError::Io {
            path: self.task_path(task_id),
            source: std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("task {task_id} not found"),
            ),
        })?;
        let len = rec.pending.len();
        let intent = rec.pending.get_mut(index).ok_or_else(|| StoreError::Io {
            path: self.task_path(task_id),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("pending intent index {index} out of range (len={len})"),
            ),
        })?;
        intent.done = true;
        self.save_task(&rec)
    }

    // Plan snapshots — written verbatim, fsynced; returns the sha256 hex.

    pub fn save_plan(&self, task_id: &str, markdown: &str) -> Result<String, StoreError> {
        use sha2::Digest;
        let path = self.plan_path(task_id);
        let bytes = markdown.as_bytes();
        write_atomic(&path, bytes)?;
        // sha2 0.11's digest output no longer implements LowerHex; map bytes.
        let digest = sha2::Sha256::digest(bytes);
        Ok(digest.iter().map(|b| format!("{b:02x}")).collect())
    }

    pub fn load_plan(&self, task_id: &str) -> Result<String, StoreError> {
        let path = self.plan_path(task_id);
        match std::fs::read(&path) {
            Ok(bytes) => String::from_utf8(bytes).map_err(|e| StoreError::Io {
                path: path.clone(),
                source: std::io::Error::new(std::io::ErrorKind::InvalidData, e),
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(StoreError::MissingPlan(task_id.to_string()))
            }
            Err(source) => Err(StoreError::Io { path, source }),
        }
    }

    // Poll cursor — the previous RepoSnapshot per forge, as opaque JSON.

    pub fn save_cursor(&self, forge: &str, snapshot: &serde_json::Value) -> Result<(), StoreError> {
        let path = self.cursor_path(forge);
        let bytes = serde_json::to_vec_pretty(snapshot).map_err(|source| StoreError::Corrupt {
            path: path.clone(),
            source,
        })?;
        write_atomic(&path, &bytes)
    }

    pub fn load_cursor(&self, forge: &str) -> Result<Option<serde_json::Value>, StoreError> {
        let path = self.cursor_path(forge);
        match std::fs::read(&path) {
            Ok(bytes) => {
                let val = serde_json::from_slice(&bytes).map_err(|source| StoreError::Corrupt {
                    path: path.clone(),
                    source,
                })?;
                Ok(Some(val))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(StoreError::Io { path, source }),
        }
    }

    // Internal path helpers.

    fn task_path(&self, id: &str) -> PathBuf {
        self.root.join("tasks").join(format!("{id}.json"))
    }

    fn plan_path(&self, task_id: &str) -> PathBuf {
        self.root.join("plans").join(format!("{task_id}.md"))
    }

    fn cursor_path(&self, forge: &str) -> PathBuf {
        self.root.join("cursor").join(format!("{forge}.json"))
    }
}

/// Write `bytes` to `path` atomically via a `.tmp` sibling + fsync + rename.
///
/// The tmp file is `<path>.tmp` (not `<stem>.tmp` — we append to the full
/// filename to avoid collisions between `foo.json` and `foo.md`).
///
/// Durability guarantees:
/// - The file itself is fsynced before the rename, so its bytes are on disk.
/// - The parent directory is fsynced after the rename, so the rename entry is
///   on disk. Without this, a kernel crash after rename but before the journal
///   flushes could leave the old file visible on next mount.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    use std::io::Write;
    let io = |source| StoreError::Io {
        path: path.to_path_buf(),
        source,
    };
    let tmp = {
        let mut os = path.as_os_str().to_owned();
        os.push(".tmp");
        PathBuf::from(os)
    };
    let mut f = std::fs::File::create(&tmp).map_err(io)?;
    f.write_all(bytes).map_err(io)?;
    f.sync_all().map_err(io)?; // fsync the file
    std::fs::rename(&tmp, path).map_err(io)?;
    // fsync the parent dir so the rename itself is durable
    if let Some(parent) = path.parent() {
        std::fs::File::open(parent)
            .and_then(|d| d.sync_all())
            .map_err(io)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::Action;
    use crate::task::{ActionIntent, TaskRecord};
    use tempfile::TempDir;

    fn store() -> (TempDir, Store) {
        let dir = TempDir::new().unwrap();
        let s = Store::open(dir.path().join(".conduit")).unwrap();
        (dir, s)
    }

    fn rec_with_pending() -> TaskRecord {
        let mut r = TaskRecord::new("ADR-0003", "3", "Adopt snapshot-diff router", "deadbeef");
        r.pending = vec![
            ActionIntent {
                action: Action::OpenPr,
                done: false,
            },
            ActionIntent {
                action: Action::ApplyPrLabels,
                done: false,
            },
        ];
        r
    }

    #[test]
    fn save_then_load_round_trips_including_pending_intents() {
        let (_d, s) = store();
        let r = rec_with_pending();
        s.save_task(&r).unwrap();
        let loaded = s.load_task(&r.id).unwrap().unwrap();
        assert_eq!(loaded.pending.len(), 2);
        assert!(!loaded.pending[0].done);
        assert_eq!(loaded.adr_reference, "ADR-0003");
    }

    #[test]
    fn load_missing_task_is_none() {
        let (_d, s) = store();
        assert!(s.load_task("nope").unwrap().is_none());
    }

    #[test]
    fn mark_intent_done_persists() {
        let (_d, s) = store();
        let r = rec_with_pending();
        s.save_task(&r).unwrap();
        s.mark_intent_done(&r.id, 0).unwrap();
        let loaded = s.load_task(&r.id).unwrap().unwrap();
        assert!(loaded.pending[0].done);
        assert!(!loaded.pending[1].done);
    }

    #[test]
    fn atomic_write_leaves_no_tmp_file_and_survives_stale_tmp() {
        let (_d, s) = store();
        let r = rec_with_pending();
        // A stale tmp from a "crash" mid-write must not break a later save/load.
        let tasks = s.root().join("tasks");
        std::fs::write(tasks.join(format!("{}.json.tmp", r.id)), b"{ partial").unwrap();
        s.save_task(&r).unwrap();
        assert!(
            !tasks.join(format!("{}.json.tmp", r.id)).exists(),
            "tmp must be renamed away"
        );
        assert!(s.load_task(&r.id).unwrap().is_some());
        // list_tasks ignores non-.json / tmp leftovers
        std::fs::write(tasks.join("other.json.tmp"), b"junk").unwrap();
        assert_eq!(s.list_tasks().unwrap().len(), 1);
    }

    #[test]
    fn plan_snapshot_round_trips_verbatim_and_returns_sha256() {
        let (_d, s) = store();
        let md = "# Plan\n\nstep one\n"; // exact bytes, incl. trailing newline
        let sha = s.save_plan("adr-0003", md).unwrap();
        assert_eq!(s.load_plan("adr-0003").unwrap(), md);
        // sha256 of the exact bytes
        use sha2::Digest;
        let want: String = sha2::Sha256::digest(md.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(sha, want);
    }

    #[test]
    fn missing_plan_is_a_typed_error() {
        let (_d, s) = store();
        assert!(matches!(
            s.load_plan("nope"),
            Err(StoreError::MissingPlan(_))
        ));
    }

    #[test]
    fn cursor_round_trips_per_forge() {
        let (_d, s) = store();
        assert!(s.load_cursor("gitea").unwrap().is_none());
        let snap = serde_json::json!({"issues": [], "prs": []});
        s.save_cursor("gitea", &snap).unwrap();
        assert_eq!(s.load_cursor("gitea").unwrap().unwrap(), snap);
        assert!(
            s.load_cursor("github").unwrap().is_none(),
            "cursors are per-forge"
        );
    }
}
