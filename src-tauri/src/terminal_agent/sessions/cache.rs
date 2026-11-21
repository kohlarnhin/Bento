use super::{unix_millis, CachedFile, Catalog, RECENT};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

const VERSION: u32 = 1;
const MAX_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Deserialize, Serialize)]
struct DiskIndex<T> {
    version: u32,
    files: T,
}

pub struct SharedCatalog {
    cache_file: Option<PathBuf>,
    snapshot: Mutex<Arc<Catalog>>,
    // Serialize scans and atomic writes. Readers can keep using the last snapshot.
    needs_write: Mutex<bool>,
}

impl SharedCatalog {
    pub fn new(home: Option<PathBuf>, data_dir: Option<&Path>) -> Self {
        let cache_file = data_dir.map(|dir| dir.join("agent-sessions.json"));
        let mut catalog = Catalog::new(home);
        let saved = cache_file.as_deref().and_then(load);
        let mut needs_write = saved.is_none();
        if let Some(mut files) = saved {
            let cutoff = unix_millis(SystemTime::now()).saturating_sub(RECENT);
            let before = files.len();
            files.retain(|_, file| unix_millis(file.modified) >= cutoff);
            needs_write = before != files.len();
            catalog.files = files;
        }
        Self {
            cache_file,
            snapshot: Mutex::new(Arc::new(catalog)),
            needs_write: Mutex::new(needs_write),
        }
    }

    pub fn snapshot(&self) -> Arc<Catalog> {
        self.snapshot
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    pub fn refresh(&self) -> Arc<Catalog> {
        let observed = self.snapshot();
        let mut needs_write = self
            .needs_write
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let latest = self.snapshot();
        if !Arc::ptr_eq(&observed, &latest) {
            // A concurrent refresh already produced a snapshot for this request.
            return latest;
        }
        let mut next = (*latest).clone();
        *needs_write |= next.refresh();
        let next = Arc::new(next);
        *self
            .snapshot
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = next.clone();
        if *needs_write {
            if let Some(path) = &self.cache_file {
                *needs_write = save(path, &next.files).is_err();
            }
        }
        next
    }
}

fn load(path: &Path) -> Option<HashMap<PathBuf, CachedFile>> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || metadata.len() > MAX_BYTES {
        return None;
    }
    let index: DiskIndex<_> = serde_json::from_reader(BufReader::new(file.take(MAX_BYTES))).ok()?;
    (index.version == VERSION).then_some(index.files)
}

fn save(path: &Path, files: &HashMap<PathBuf, CachedFile>) -> io::Result<()> {
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let directory = path
        .parent()
        .ok_or_else(|| io::Error::other("Missing index directory"))?;
    DirBuilder::new()
        .mode(0o700)
        .recursive(true)
        .create(directory)?;
    let temp = path.with_extension(format!(
        "{}-{}.tmp",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    let file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&temp)?;
    let result = (|| {
        let mut writer = BufWriter::new(file);
        serde_json::to_writer(
            &mut writer,
            &DiskIndex {
                version: VERSION,
                files,
            },
        )?;
        writer.flush()?;
        writer.get_ref().sync_all()?;
        fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}
