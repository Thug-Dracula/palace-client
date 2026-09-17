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

/// Save fetched bytes under their base name in `dir`, refusing path escapes and
/// names that cannot be a legal filename on every platform we support.
pub fn write_media(dir: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf> {
    let base = Path::new(name)
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty())
        .filter(|n| legal_file_name(n))
        .ok_or_else(|| ClientError::Config(format!("unusable media name {name:?}")))?;
    let path = dir.join(base);
    std::fs::write(&path, bytes).map_err(ClientError::Io)?;
    Ok(path)
}

/// Whether `base` can name a file on every platform this client targets.
///
/// Enforced everywhere rather than only on Windows: the name is what the renderer
/// later looks the file up by, so a name that works on one platform and not another
/// is a bug wherever it runs. Windows forbids `: * ? " < > |` and control
/// characters, strips a trailing dot or space, and reserves `CON`, `PRN`, `AUX`,
/// `NUL`, `COM1`-`COM9` and `LPT1`-`LPT9` as device names even with an extension,
/// so `aux.png` is not a file there. Escaping instead of refusing would change the
/// name the renderer looks up, turning a crash into a silently missing image.
fn legal_file_name(base: &str) -> bool {
    let forbidden = |c: char| matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|') || c < ' ';
    !base.is_empty()
        && !base.chars().any(forbidden)
        && !base.ends_with('.')
        && !base.ends_with(' ')
        && !is_reserved_device_name(base)
}

fn is_reserved_device_name(base: &str) -> bool {
    let stem = base.split('.').next().unwrap_or(base).to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_a_windows_filesystem_cannot_hold_is_refused() {
        for bad in [
            "a:b.gif",
            "a*b.gif",
            "a?b.gif",
            "a\"b.gif",
            "a<b.gif",
            "a>b.gif",
            "a|b.gif",
            "a\u{1}b.gif",
            "con.gif",
            "AUX.png",
            "nul",
            "com1.bin",
            "lpt9.gif",
            "trailing.",
            "trailing ",
            "",
        ] {
            assert!(!legal_file_name(bad), "{bad:?} must be refused");
        }
    }

    #[test]
    fn an_ordinary_media_name_is_kept() {
        for good in [
            "bg-x.jpg",
            "rainy-day.gif",
            "a.b.c.png",
            "apdata__media__bg.jpg",
            "COM0.png",
            "console.png",
            "com.gif",
        ] {
            assert!(legal_file_name(good), "{good:?} must be allowed");
        }
    }

    #[test]
    fn writing_an_unusable_name_errors_instead_of_writing_it() {
        let dir = std::env::temp_dir().join(format!("palace-media-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");

        let refused = write_media(&dir, "aux.gif", b"bytes");
        assert!(
            matches!(&refused, Err(ClientError::Config(_))),
            "expected a config error, got {refused:?}"
        );
        assert!(
            !dir.join("aux.gif").exists(),
            "a refused name must not be written"
        );

        let written = write_media(&dir, "bg-x.jpg", b"bytes").expect("a legal name is written");
        assert!(written.ends_with("bg-x.jpg"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
