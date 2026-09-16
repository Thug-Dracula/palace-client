//! Live asset intake: media fetched over HTTP, props received by transfer.
//!
//! Rooms name their background and overlays; the server publishes the media base
//! URL. Fetching is blocking HTTP, so it runs on a worker thread and the result
//! is handed back as a file path. Props arrive through `palace-asset`'s pipeline
//! and are inserted into the render store directly.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::time::Instant;

use palace_asset::{MediaCache, MediaConfig, MediaFetcher, UreqTransport};
use palace_render::{MediaStore, PropStore};

use crate::error::{ClientError, Result};

/// A media file to fetch.
#[derive(Debug, Clone)]
pub struct MediaJob {
    pub base_url: String,
    pub name: String,
}

/// The outcome of one media job.
#[derive(Debug, Clone)]
pub enum MediaResult {
    Fetched { name: String, path: PathBuf },
    Failed { name: String, error: String },
}

/// The session's write-through asset directory and the names already asked for.
#[derive(Debug)]
pub struct AssetWorkspace {
    media_dir: PathBuf,
    props_dir: PathBuf,
    requested_media: HashSet<String>,
}

impl AssetWorkspace {
    /// Create the session directories under `root`.
    pub fn new(root: &Path) -> Result<Self> {
        let media_dir = root.join("media");
        let props_dir = root.join("props");
        std::fs::create_dir_all(&media_dir).map_err(ClientError::Io)?;
        std::fs::create_dir_all(&props_dir).map_err(ClientError::Io)?;
        Ok(AssetWorkspace {
            media_dir,
            props_dir,
            requested_media: HashSet::new(),
        })
    }

    /// The directory fetched media is written to.
    #[must_use]
    pub fn media_dir(&self) -> &Path {
        &self.media_dir
    }

    /// The directory received props are written to.
    #[must_use]
    pub fn props_dir(&self) -> &Path {
        &self.props_dir
    }

    /// Remember that a media name has been queued, so it is not queued twice.
    pub fn mark_requested(&mut self, name: &str) {
        self.requested_media.insert(name.to_ascii_lowercase());
    }

    /// Whether a media name has already been queued.
    #[must_use]
    pub fn is_requested(&self, name: &str) -> bool {
        self.requested_media.contains(&name.to_ascii_lowercase())
    }
}

/// Media names neither present in `store` nor already queued.
#[must_use]
pub fn missing_media(
    store: &MediaStore,
    workspace: &AssetWorkspace,
    names: &[String],
) -> Vec<String> {
    let mut out = Vec::new();
    for name in names {
        if name.is_empty() || store.resolve(name).is_some() || workspace.is_requested(name) {
            continue;
        }
        out.push(name.clone());
    }
    out
}

/// Distinct prop ids absent from `store`, in first-seen order.
#[must_use]
pub fn missing_props(store: &PropStore, ids: &[u32]) -> Vec<u32> {
    let mut seen = HashSet::new();
    ids.iter()
        .copied()
        .filter(|id| *id != 0)
        .filter(|id| seen.insert(*id))
        .filter(|id| !store.contains(*id))
        .collect()
}

/// Save fetched bytes under their base name in `dir`, refusing path escapes.
pub fn write_media(dir: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf> {
    let base = Path::new(name)
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty())
        .ok_or_else(|| ClientError::Config(format!("unsafe media name {name:?}")))?;
    let path = dir.join(base);
    std::fs::write(&path, bytes).map_err(ClientError::Io)?;
    Ok(path)
}

/// Run the blocking media fetch loop until the job channel closes.
pub fn run_media_worker(dir: PathBuf, jobs: Receiver<MediaJob>, out: Sender<MediaResult>) {
    let config = MediaConfig {
        user_agent: format!("palace-client/{}", env!("CARGO_PKG_VERSION")),
        ..MediaConfig::default()
    };
    let transport = UreqTransport::from_config(&config);
    let cache = MediaCache::new(dir.join("http-cache"));
    let mut fetcher = MediaFetcher::new(transport, cache, config);
    let started = Instant::now();

    while let Ok(job) = jobs.recv() {
        let now_ms = started.elapsed().as_millis() as u64;
        let result = match fetcher.fetch(&job.base_url, &job.name, now_ms) {
            Ok(fetch) => match write_media(&dir, &job.name, &fetch.bytes) {
                Ok(path) => MediaResult::Fetched {
                    name: job.name,
                    path,
                },
                Err(e) => MediaResult::Failed {
                    name: job.name,
                    error: e.to_string(),
                },
            },
            Err(e) => MediaResult::Failed {
                name: job.name,
                error: e.to_string(),
            },
        };
        if out.send(result).is_err() {
            break;
        }
    }
}
