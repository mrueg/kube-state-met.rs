//! File watching for config hot reload (Go: `internal/watch.go` and the
//! `movableWatch` of `internal/wrapper.go`).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use notify::event::{EventKind, ModifyKind, RenameMode};
use notify::{RecursiveMode, Watcher};
use parking_lot::Mutex;
use tokio::sync::mpsc::UnboundedSender;

use crate::server::clean_path;

/// `readYAMLFile`: reads `path` and checks that it holds a YAML mapping.
pub fn read_yaml_file(path: &str) -> Result<(), std::io::Error> {
    let b = std::fs::read(clean_path(path))?;
    let text = String::from_utf8_lossy(&b);
    if text.trim().is_empty() {
        return Ok(());
    }
    serde_yaml_ng::from_str::<Option<serde_yaml_ng::Mapping>>(&text)
        .map(|_| ())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
}

fn real_path(p: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(p).ok()
}

/// A running watch; dropping it stops the watch.
pub struct FileWatch {
    _watcher: notify::RecommendedWatcher,
}

/// `watchFile`: sends the changed path to `tx` whenever the file at `path`
/// is written or created, or its symlink target changes (ConfigMap
/// updates swap `..data`), until the file is removed or the watch is
/// dropped. It watches the parent directory.
pub fn watch_file(path: &str, tx: UnboundedSender<String>) -> Result<FileWatch, String> {
    // Absolute, so it compares equal to the paths the events carry.
    let file = PathBuf::from(clean_path(path));
    let file = if file.is_absolute() {
        file
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(file)
    };
    let dir = file
        .parent()
        .map_or_else(|| PathBuf::from("/"), Path::to_path_buf);
    let real = Arc::new(Mutex::new(real_path(&file)));
    let stopped = Arc::new(AtomicBool::new(false));
    let target = file.clone();
    let handler = move |res: notify::Result<notify::Event>| {
        if stopped.load(Ordering::SeqCst) {
            return;
        }
        let ev = match res {
            Ok(ev) => ev,
            Err(e) => {
                tracing::error!(error = %e, file = %target.display(), "Error watching file");
                return;
            }
        };
        let is_file = ev.paths.contains(&target);
        let write_or_create = matches!(
            ev.kind,
            EventKind::Create(_)
                | EventKind::Modify(ModifyKind::Data(_) | ModifyKind::Any)
                | EventKind::Modify(ModifyKind::Name(RenameMode::To | RenameMode::Both))
        ) && ev.paths.last().is_some_and(|p| *p == target);
        let current = real_path(&target);
        let mut r = real.lock();
        let moved = current.is_some() && current != *r;
        if (is_file && write_or_create) || moved {
            *r = current;
            let name = ev
                .paths
                .last()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            let _ = tx.send(name);
        } else if is_file
            && matches!(
                ev.kind,
                EventKind::Remove(_) | EventKind::Modify(ModifyKind::Name(RenameMode::From))
            )
        {
            stopped.store(true, Ordering::SeqCst);
        }
    };
    let mut w = notify::recommended_watcher(handler).map_err(|e| e.to_string())?;
    w.watch(&dir, RecursiveMode::NonRecursive)
        .map_err(|e| e.to_string())?;
    Ok(FileWatch { _watcher: w })
}

/// `movableWatch`: one watched file whose path can change between runs.
pub struct MovableWatch {
    name: &'static str,
    file: String,
    watch: Option<FileWatch>,
}

impl MovableWatch {
    pub fn new(name: &'static str) -> Self {
        MovableWatch {
            name,
            file: String::new(),
            watch: None,
        }
    }

    /// Moves the watch to `file` unless it already follows it; an empty
    /// file stops it.
    pub fn watch(&mut self, file: &str, tx: &UnboundedSender<String>) {
        if file == self.file {
            return;
        }
        self.watch = None;
        file.clone_into(&mut self.file);
        if file.is_empty() {
            return;
        }
        match watch_file(file, tx.clone()) {
            Ok(w) => self.watch = Some(w),
            Err(e) => {
                tracing::error!(error = %e, file, "Failed to watch {}", self.name);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sees_writes_and_atomic_saves() {
        let dir = std::env::temp_dir().join(format!("ksm-reload-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("config.yaml");
        std::fs::write(&f, "a: 1\n").unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let _w = watch_file(&f.to_string_lossy(), tx).unwrap();
        std::fs::write(dir.join("other.yaml"), "x").unwrap();
        std::fs::write(&f, "a: 2\n").unwrap();
        let got = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got, f.display().to_string());
        while rx.try_recv().is_ok() {}
        let tmp = dir.join("config.yaml.tmp");
        std::fs::write(&tmp, "a: 3\n").unwrap();
        std::fs::rename(&tmp, &f).unwrap();
        let got = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got, f.display().to_string());
        assert!(read_yaml_file(&f.to_string_lossy()).is_ok());
        std::fs::write(&f, "- a\n").unwrap();
        assert!(read_yaml_file(&f.to_string_lossy()).is_err());
    }
}
