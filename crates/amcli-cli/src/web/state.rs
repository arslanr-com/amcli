//! The model the page is looking at, and how it follows the file.
//!
//! The viewer is read-only, but the file is not: an agent edits it through
//! amcli, a person saves it from Archi. Rather than a watcher thread, the
//! state re-checks the file's size and modification time on demand — at most
//! once a second, whichever request comes first — and re-opens it when they
//! move. A file caught mid-write fails to parse; that keeps the last good
//! snapshot and remembers the failure so the page can say so, and the next
//! change to the file is tried again.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Instant, SystemTime};

use amcli_model::Model;

/// Everything derived from one successful load. Immutable once built, shared
/// by whichever requests are in flight, dropped when the last one finishes.
pub struct Snapshot {
    pub model: Model,
    pub checksum: String,
    /// The `/api/model` body, built once here rather than per request.
    pub model_json: Arc<str>,
    /// Modification time and length of the file this was read from.
    stamp: Stamp,
    /// When it was loaded, as seconds since the epoch, for the page to show.
    pub loaded: u64,
}

type Stamp = (Option<SystemTime>, u64);

pub struct State {
    pub path: PathBuf,
    pub port: u16,
    /// Host headers accepted besides the loopback ones, already lower-cased.
    /// Empty unless `--allow-host` named a reverse proxy's name.
    pub allow_hosts: Vec<String>,
    snap: RwLock<Arc<Snapshot>>,
    gate: Mutex<Gate>,
}

struct Gate {
    last_check: Instant,
    /// The stamp of the last file we failed to open, so a broken file is
    /// reported once rather than re-parsed on every request.
    failed: Option<Stamp>,
    last_error: Option<String>,
}

/// How often the file is looked at, at most.
const CHECK_EVERY: std::time::Duration = std::time::Duration::from_secs(1);

impl State {
    pub fn new(model: Model, path: PathBuf, port: u16, allow_hosts: Vec<String>) -> State {
        let stamp = stamp(&path);
        let snap = Snapshot::build(model, stamp);
        State {
            path,
            port,
            allow_hosts,
            snap: RwLock::new(Arc::new(snap)),
            gate: Mutex::new(Gate { last_check: Instant::now(), failed: None, last_error: None }),
        }
    }

    /// The current snapshot, after a cheap check that the file has not moved.
    pub fn current(&self) -> Arc<Snapshot> {
        self.refresh_if_due();
        Arc::clone(&self.snap.read().unwrap_or_else(|e| e.into_inner()))
    }

    /// The last failure to re-open the file, if the file is currently broken.
    pub fn last_error(&self) -> Option<String> {
        self.gate.lock().unwrap_or_else(|e| e.into_inner()).last_error.clone()
    }

    fn refresh_if_due(&self) {
        let mut gate = self.gate.lock().unwrap_or_else(|e| e.into_inner());
        if gate.last_check.elapsed() < CHECK_EVERY {
            return;
        }
        gate.last_check = Instant::now();
        let now = stamp(&self.path);
        let unchanged = self.snap.read().unwrap_or_else(|e| e.into_inner()).stamp == now;
        if unchanged || gate.failed == Some(now) {
            return;
        }
        match Model::open(&self.path) {
            Ok(model) => {
                let fresh = Arc::new(Snapshot::build(model, now));
                *self.snap.write().unwrap_or_else(|e| e.into_inner()) = fresh;
                gate.failed = None;
                gate.last_error = None;
            }
            Err(e) => {
                gate.failed = Some(now);
                gate.last_error = Some(e.to_string());
            }
        }
    }

    /// Force the next `current()` to look at the file, for tests that would
    /// otherwise wait out the check interval.
    #[cfg(test)]
    pub fn expire(&self) {
        self.gate.lock().unwrap_or_else(|e| e.into_inner()).last_check =
            Instant::now() - CHECK_EVERY * 2;
    }
}

impl Snapshot {
    fn build(model: Model, stamp: Stamp) -> Snapshot {
        let checksum = model.checksum().unwrap_or_default();
        let model_json = super::api::model_json(&model, &checksum).into();
        let loaded = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Snapshot { model, checksum, model_json, stamp, loaded }
    }
}

fn stamp(path: &Path) -> Stamp {
    match std::fs::metadata(path) {
        Ok(m) => (m.modified().ok(), m.len()),
        Err(_) => (None, 0),
    }
}
