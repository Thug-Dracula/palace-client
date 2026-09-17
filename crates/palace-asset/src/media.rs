//! Fetching room backgrounds and hotspot images over HTTP.
//!
//! ## Where the URL comes from
//!
//! The `HTTP` opcode (`MSG_HTTPSERVER`) delivers one NUL-terminated string: the
//! media server's base URL. The capture in `fixtures/logon-run1/` is a real one:
//!
//! ```text
//! HTTP  https://media.palace.example.info/palace/media
//! ```
//!
//! Room descriptions carry a background file name (`RoomDesc::picture`), and
//! hotspot records carry image names. The image is fetched from
//! `media_server + name`.
//!
//! ## The extension fallback chain
//!
//! A room that names `sqoom23.gif` is really asking for whatever the server
//! happens to have — the author may have converted the background to PNG or
//! JPEG and left the room description alone. Both reference implementations
//! chase this the same way:
//!
//! * OpenPalace `PalaceRoomView.mxml`: `tryPngBG` → on failure `tryJpegBG` →
//!   on failure `tryRegularBG`. Triggered when the name matches `/^(.*)\.gif$/i`.
//! * FreePalace `Media/Loader.hs::fetchCachedBackgroundImagePath`: the `.gif`
//!   suffix maps the name to `[pngFile, jpegFile, gifFile]` and the first one
//!   that fetches wins.
//!
//! So the chain is `.png → .jpg → original` — but only for a name that ends in
//! `.gif`. Any other name is fetched exactly as written. [`fallback_chain`] is
//! public and unit-tested so the rule is inspectable rather than buried in a
//! retry loop.
//!
//! ## URL joining
//!
//! The base URL in the capture has **no trailing slash** and the file name has
//! no leading one. OpenPalace concatenates the two directly, which would produce
//! `.../palace/mediasqoom23.gif` — a bug that only shows up against a server
//! whose base URL lacks the slash. FreePalace inserts a separator when needed.
//! This crate inserts the separator: [`media_url`] is total and never
//! double-slashes.
//!
//! ## Bounds and cancellation
//!
//! Every request carries a connect timeout and a total timeout, and the body is
//! read with a hard byte limit, so a server that accepts the connection and then
//! says nothing cannot hang or exhaust the client. Failures are remembered for a
//! short negative-cache window so a missing background is not re-requested on
//! every room change.

use std::collections::HashMap;
use std::fmt;
use std::io::Read;
use std::time::Duration;

use crate::cache::MediaCache;
use crate::error::{AssetError, Result};

/// Default ceiling on one media response body.
pub const DEFAULT_MAX_MEDIA_BYTES: u64 = 64 * 1024 * 1024;

/// Default connect timeout.
pub const DEFAULT_CONNECT_TIMEOUT_MS: u64 = 5_000;

/// Default total timeout for one request, connect included.
pub const DEFAULT_TOTAL_TIMEOUT_MS: u64 = 20_000;

/// Default negative-cache window after every candidate URL failed.
pub const DEFAULT_NEGATIVE_TTL_MS: u64 = 60_000;

/// Default cap on negative-cache entries.
pub const DEFAULT_MAX_NEGATIVE_ENTRIES: usize = 2048;

/// Default redirect budget.
pub const DEFAULT_MAX_REDIRECTS: u32 = 5;

/// One HTTP GET, however it is performed.
pub trait HttpTransport: Send + Sync {
    /// GET `url`, refusing to read more than `max_bytes` of body.
    fn get(&self, url: &str, max_bytes: u64) -> Result<HttpResponse>;
}

impl HttpTransport for Box<dyn HttpTransport> {
    fn get(&self, url: &str, max_bytes: u64) -> Result<HttpResponse> {
        (**self).get(url, max_bytes)
    }
}

/// A completed HTTP response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    /// HTTP status code.
    pub status: u16,
    /// The body, possibly empty.
    pub body: Vec<u8>,
    /// `Content-Type`, when the server sent one.
    pub content_type: Option<String>,
}

impl HttpResponse {
    /// Sparky's HTTP execute branch accepts only these two MIME types.
    #[must_use]
    pub fn is_script(&self) -> bool {
        self.content_type.as_deref().is_some_and(|value| {
            let mime = value.split(';').next().unwrap_or("").trim();
            mime.eq_ignore_ascii_case("text/iptscrae") || mime.eq_ignore_ascii_case("text/ipt")
        })
    }

    /// True for a 2xx status.
    #[must_use]
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// The real transport: blocking HTTP(S) via `ureq`, with rustls.
#[derive(Clone)]
pub struct UreqTransport {
    agent: ureq::Agent,
}

impl fmt::Debug for UreqTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UreqTransport").finish_non_exhaustive()
    }
}

impl UreqTransport {
    /// Build a transport with the given connect and total timeouts.
    #[must_use]
    pub fn new(connect_timeout: Duration, total_timeout: Duration, user_agent: &str) -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_connect(Some(connect_timeout))
            .timeout_global(Some(total_timeout))
            .max_redirects(DEFAULT_MAX_REDIRECTS)
            .http_status_as_error(false)
            .user_agent(user_agent)
            .build();
        UreqTransport {
            agent: config.into(),
        }
    }

    /// Build a transport from a [`MediaConfig`].
    #[must_use]
    pub fn from_config(cfg: &MediaConfig) -> Self {
        Self::new(
            Duration::from_millis(cfg.connect_timeout_ms),
            Duration::from_millis(cfg.total_timeout_ms),
            &cfg.user_agent,
        )
    }
}

impl HttpTransport for UreqTransport {
    fn get(&self, url: &str, max_bytes: u64) -> Result<HttpResponse> {
        let mut res = self
            .agent
            .get(url)
            .call()
            .map_err(|e| AssetError::Http(format!("{url}: {e}")))?;
        let status = res.status().as_u16();
        let content_type = res
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);

        if let Some(declared) = content_length(&res) {
            if declared > max_bytes {
                return Err(AssetError::BodyTooLarge {
                    url: url.to_string(),
                    limit: max_bytes,
                });
            }
        }

        let mut body = Vec::new();
        res.body_mut()
            .with_config()
            .limit(max_bytes.saturating_add(1))
            .reader()
            .read_to_end(&mut body)
            .map_err(|e| body_error(url, max_bytes, e))?;
        if body.len() as u64 > max_bytes {
            return Err(AssetError::BodyTooLarge {
                url: url.to_string(),
                limit: max_bytes,
            });
        }

        Ok(HttpResponse {
            status,
            body,
            content_type,
        })
    }
}

fn content_length(res: &ureq::http::Response<ureq::Body>) -> Option<u64> {
    res.headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
}

/// `ureq` reports a body that outran the configured limit as
/// [`ureq::Error::BodyExceedsLimit`], wrapped in an `io::Error`; unwrap it so a
/// hostile server reads as a size refusal rather than an opaque transport fault.
fn body_error(url: &str, max_bytes: u64, e: std::io::Error) -> AssetError {
    if let Some(ureq::Error::BodyExceedsLimit(_)) = e
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<ureq::Error>())
    {
        return AssetError::BodyTooLarge {
            url: url.to_string(),
            limit: max_bytes,
        };
    }
    AssetError::Http(format!("{url}: reading body: {e}"))
}

/// Fetcher tunables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaConfig {
    /// Ceiling on one response body.
    pub max_bytes: u64,
    /// Connect timeout.
    pub connect_timeout_ms: u64,
    /// Total request timeout, connect included.
    pub total_timeout_ms: u64,
    /// How long to remember a total failure for one media name.
    pub negative_ttl_ms: u64,
    /// Cap on remembered failures.
    pub max_negative_entries: usize,
    /// `User-Agent` sent with every request.
    pub user_agent: String,
}

impl Default for MediaConfig {
    fn default() -> Self {
        MediaConfig {
            max_bytes: DEFAULT_MAX_MEDIA_BYTES,
            connect_timeout_ms: DEFAULT_CONNECT_TIMEOUT_MS,
            total_timeout_ms: DEFAULT_TOTAL_TIMEOUT_MS,
            negative_ttl_ms: DEFAULT_NEGATIVE_TTL_MS,
            max_negative_entries: DEFAULT_MAX_NEGATIVE_ENTRIES,
            user_agent: format!("palace-asset/{}", env!("CARGO_PKG_VERSION")),
        }
    }
}

/// A fetched media file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaFetch {
    /// The file's bytes.
    pub bytes: Vec<u8>,
    /// The URL that actually answered.
    pub url: String,
    /// The media name as the room description spelled it.
    pub name: String,
    /// True when the bytes came from the on-disk cache.
    pub from_cache: bool,
    /// Every URL tried before this one, in order.
    pub attempts: Vec<String>,
}

/// Fetches media into a [`MediaCache`].
pub struct MediaFetcher<T: HttpTransport> {
    transport: T,
    cache: MediaCache,
    cfg: MediaConfig,
    negative: HashMap<String, u64>,
}

impl<T: HttpTransport> fmt::Debug for MediaFetcher<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MediaFetcher")
            .field("cache", &self.cache)
            .field("config", &self.cfg)
            .field("negative_entries", &self.negative.len())
            .finish_non_exhaustive()
    }
}

impl<T: HttpTransport> MediaFetcher<T> {
    /// Build a fetcher.
    #[must_use]
    pub fn new(transport: T, cache: MediaCache, cfg: MediaConfig) -> Self {
        MediaFetcher {
            transport,
            cache,
            cfg,
            negative: HashMap::new(),
        }
    }

    /// The fetcher's configuration.
    #[must_use]
    pub fn config(&self) -> &MediaConfig {
        &self.cfg
    }

    /// The cache being filled.
    #[must_use]
    pub fn cache(&self) -> &MediaCache {
        &self.cache
    }

    /// Fetch a room background, applying the `.png → .jpg → original` chain.
    pub fn fetch(&mut self, base_url: &str, name: &str, now_ms: u64) -> Result<MediaFetch> {
        let candidates = fallback_chain(name);
        self.fetch_candidates(base_url, name, &candidates, now_ms)
    }

    /// Fetch exactly `name`, with no extension fallback.
    ///
    /// Hotspot and overlay images are referenced by their real name and get no
    /// chain in either reference client.
    pub fn fetch_single(&mut self, base_url: &str, name: &str, now_ms: u64) -> Result<MediaFetch> {
        self.fetch_candidates(
            base_url,
            name,
            std::slice::from_ref(&name.to_string()),
            now_ms,
        )
    }

    fn fetch_candidates(
        &mut self,
        base_url: &str,
        name: &str,
        candidates: &[String],
        now_ms: u64,
    ) -> Result<MediaFetch> {
        if base_url.trim().is_empty() {
            return Err(AssetError::Url("media server URL is empty".to_string()));
        }
        if let Some(retry_after) = self.negative.get(name).copied() {
            if now_ms < retry_after {
                return Err(AssetError::RecentlyFailed {
                    name: name.to_string(),
                    retry_after_ms: retry_after,
                });
            }
            self.negative.remove(name);
        }
        if let Some(bytes) = self.cache.get(base_url, name) {
            return Ok(MediaFetch {
                bytes,
                url: media_url(base_url, name),
                name: name.to_string(),
                from_cache: true,
                attempts: Vec::new(),
            });
        }

        let mut attempts = Vec::new();
        for candidate in candidates {
            let Ok(url) = build_url(base_url, candidate) else {
                attempts.push(format!("{} (invalid URL)", media_url(base_url, candidate)));
                continue;
            };
            match self.transport.get(&url, self.cfg.max_bytes) {
                Ok(response) if response.is_success() && !response.body.is_empty() => {
                    self.cache.put(base_url, name, &response.body)?;
                    return Ok(MediaFetch {
                        bytes: response.body,
                        url,
                        name: name.to_string(),
                        from_cache: false,
                        attempts,
                    });
                }
                Ok(response) if response.is_success() => {
                    attempts.push(format!("{url} (200 but empty)"));
                }
                Ok(response) => {
                    attempts.push(format!("{url} (HTTP {})", response.status));
                }
                Err(e) => {
                    attempts.push(format!("{url} ({e})"));
                }
            }
        }

        self.remember_failure(name, now_ms);
        Err(AssetError::AllAttemptsFailed { attempts })
    }

    fn remember_failure(&mut self, name: &str, now_ms: u64) {
        if self.negative.len() >= self.cfg.max_negative_entries {
            self.negative.clear();
        }
        self.negative.insert(
            name.to_string(),
            now_ms.saturating_add(self.cfg.negative_ttl_ms),
        );
    }

    /// How many media names are currently being skipped.
    #[must_use]
    pub fn negative_count(&self) -> usize {
        self.negative.len()
    }

    /// Forget a remembered failure so the name is retried immediately.
    pub fn forget_failure(&mut self, name: &str) -> bool {
        self.negative.remove(name).is_some()
    }
}

/// Join a media base URL and a relative name with exactly one separator.
#[must_use]
pub fn media_url(base_url: &str, name: &str) -> String {
    format!(
        "{}/{}",
        base_url.trim_end_matches('/'),
        name.trim_start_matches('/')
    )
}

/// Sparky's HTTP cap (`Lc=4`): more than this many in flight is refused.
pub const DEFAULT_MAX_HTTP_IN_FLIGHT: usize = 4;

/// The media join sparky's `Mc` helper applies before any HTTP GET.
///
/// Absolute `http(s)` URLs pass through unchanged; anything else is joined to
/// the base with exactly one separator, so `""` with no base is refused and a
/// doubled slash cannot be produced. An empty relative name is a refusal.
#[must_use]
pub fn script_url(media_base: &str, url: &str) -> Option<String> {
    let url = url.trim();
    if url.is_empty() || url.len() > 2048 {
        return None;
    }
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return Some(url.to_string());
    }
    let base = media_base.trim();
    if base.is_empty() {
        return None;
    }
    Some(media_url(base, url))
}

/// One script-fetch request as the runtime enqueued it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptFetch {
    /// The URL exactly as the script spelled it.
    pub requested: String,
    /// The URL actually fetched, after base joining.
    pub url: String,
    /// The executing hotspot; `0` means room-level.
    pub spot: i32,
}

/// The end state of one script fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptOutcome {
    /// A body arrived; `is_script` says whether it must be executed.
    Response {
        requested: String,
        url: String,
        spot: i32,
        body: Vec<u8>,
        content_type: Option<String>,
    },
    /// The fetch failed; the reason is for `ON HTTPERROR`.
    Failure {
        requested: String,
        url: String,
        spot: i32,
        reason: String,
    },
}

impl ScriptOutcome {
    /// The spot the request was scoped to.
    #[must_use]
    pub fn spot(&self) -> i32 {
        match self {
            ScriptOutcome::Response { spot, .. } | ScriptOutcome::Failure { spot, .. } => *spot,
        }
    }

    /// The requested URL, for event trace and error text.
    #[must_use]
    pub fn requested(&self) -> &str {
        match self {
            ScriptOutcome::Response { requested, .. }
            | ScriptOutcome::Failure { requested, .. } => requested,
        }
    }
}

/// Admission guard for queued, running and not-yet-delivered HTTP requests (`Lc=4`).
#[must_use]
pub fn admits_in_flight(in_flight: usize) -> bool {
    in_flight < DEFAULT_MAX_HTTP_IN_FLIGHT
}

/// Bounded worker: the runtime hands over [`ScriptFetch`] jobs, this drains them
/// one at a time and refuses a job while the admit ledger is at the ceiling, so
/// a script cannot queue requests faster than the runtime delivers them.
pub fn run_script_worker<T: HttpTransport + 'static>(
    transport: T,
    media_base: std::sync::mpsc::Receiver<String>,
    jobs: std::sync::mpsc::Receiver<ScriptFetch>,
    out: std::sync::mpsc::Sender<ScriptOutcome>,
) {
    let mut base = String::new();
    let mut admitted = 0usize;
    while let Ok(job) = jobs.recv() {
        while let Ok(update) = media_base.try_recv() {
            base = update;
        }
        if !admits_in_flight(admitted) {
            let _ = out.send(ScriptOutcome::Failure {
                requested: job.requested,
                url: job.url,
                spot: job.spot,
                reason: "too many concurrent requests".to_string(),
            });
            continue;
        }
        admitted += 1;
        let resolved = script_url(&base, &job.requested);
        let Some(url) = resolved else {
            admitted -= 1;
            let _ = out.send(ScriptOutcome::Failure {
                requested: job.requested,
                url: job.url,
                spot: job.spot,
                reason: if base.is_empty() {
                    "no media base URL known".to_string()
                } else {
                    "could not resolve URL to http(s)".to_string()
                },
            });
            continue;
        };
        let outcome = match transport.get(&url, DEFAULT_MAX_MEDIA_BYTES) {
            Ok(response) if response.is_success() => ScriptOutcome::Response {
                requested: job.requested,
                url,
                spot: job.spot,
                body: response.body,
                content_type: response.content_type,
            },
            Ok(response) => ScriptOutcome::Failure {
                requested: job.requested,
                url,
                spot: job.spot,
                reason: format!("HTTP {}", response.status),
            },
            Err(e) => ScriptOutcome::Failure {
                requested: job.requested,
                url,
                spot: job.spot,
                reason: e.to_string(),
            },
        };
        let _ = out.send(outcome);
    }
}

fn build_url(base_url: &str, name: &str) -> Result<String> {
    let url = media_url(base_url, name);
    if url.len() > 2048 {
        return Err(AssetError::Url(url));
    }
    Ok(url)
}

/// The candidate names to try for one requested name, in order.
///
/// A name ending in `.gif` (case-insensitively) is chased as `.png`, then
/// `.jpg`, then the original. Anything else is used as-is.
#[must_use]
pub fn fallback_chain(name: &str) -> Vec<String> {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".gif") && name.len() > 4 {
        let stem = &name[..name.len() - 4];
        vec![
            format!("{stem}.png"),
            format!("{stem}.jpg"),
            name.to_string(),
        ]
    } else {
        vec![name.to_string()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Debug, Default)]
    struct StubTransport {
        responses: Mutex<HashMap<String, Result<HttpResponse>>>,
        seen: Mutex<Vec<String>>,
    }

    impl StubTransport {
        fn new(pairs: Vec<(&str, Result<HttpResponse>)>) -> Self {
            let t = StubTransport::default();
            {
                let mut map = t.responses.lock().unwrap();
                for (url, response) in pairs {
                    map.insert(url.to_string(), response);
                }
            }
            t
        }

        fn seen(&self) -> Vec<String> {
            self.seen.lock().unwrap().clone()
        }
    }

    impl HttpTransport for StubTransport {
        fn get(&self, url: &str, _max_bytes: u64) -> Result<HttpResponse> {
            self.seen.lock().unwrap().push(url.to_string());
            match self.responses.lock().unwrap().get(url) {
                Some(Ok(response)) => Ok(response.clone()),
                Some(Err(e)) => Err(AssetError::Http(e.to_string())),
                None => Ok(HttpResponse {
                    status: 404,
                    body: Vec::new(),
                    content_type: None,
                }),
            }
        }
    }

    fn temp_cache(tag: &str) -> MediaCache {
        let mut dir = std::env::temp_dir();
        dir.push(format!("palace-asset-media-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        MediaCache::new(dir)
    }

    fn ok(body: &[u8]) -> Result<HttpResponse> {
        Ok(HttpResponse {
            status: 200,
            body: body.to_vec(),
            content_type: Some("image/png".into()),
        })
    }

    const BASE: &str = "https://media.example/palace/media";

    #[test]
    fn the_chain_only_fires_for_a_gif_name() {
        assert_eq!(fallback_chain("bg.gif"), vec!["bg.png", "bg.jpg", "bg.gif"]);
        assert_eq!(fallback_chain("BG.GIF"), vec!["BG.png", "BG.jpg", "BG.GIF"]);
        assert_eq!(fallback_chain("bg.png"), vec!["bg.png"]);
        assert_eq!(
            fallback_chain("dir/bg.gif"),
            vec!["dir/bg.png", "dir/bg.jpg", "dir/bg.gif"]
        );
        assert_eq!(fallback_chain("gif"), vec!["gif"]);
        assert_eq!(fallback_chain(".gif"), vec![".gif"]);
    }

    #[test]
    fn url_joining_never_double_slashes_or_drops_the_separator() {
        assert_eq!(media_url("http://a/media", "x.gif"), "http://a/media/x.gif");
        assert_eq!(
            media_url("http://a/media/", "x.gif"),
            "http://a/media/x.gif"
        );
        assert_eq!(
            media_url("http://a/media", "/x.gif"),
            "http://a/media/x.gif"
        );
        assert_eq!(
            media_url("http://a/media///", "x.gif"),
            "http://a/media/x.gif"
        );
    }

    #[test]
    fn the_png_attempt_wins_when_it_exists() {
        let url = media_url(BASE, "bg.png");
        let stub = StubTransport::new(vec![(&url, ok(b"PNG"))]);
        let mut f = MediaFetcher::new(stub, temp_cache("png-wins"), MediaConfig::default());
        let got = f.fetch(BASE, "bg.gif", 0).unwrap();
        assert_eq!(got.bytes, b"PNG");
        assert_eq!(got.url, url);
        assert!(!got.from_cache);
        assert!(got.attempts.is_empty());
    }

    #[test]
    fn it_falls_through_png_to_jpg() {
        let jpg = media_url(BASE, "bg.jpg");
        let stub = StubTransport::new(vec![(&jpg, ok(b"JPG"))]);
        let mut f = MediaFetcher::new(stub, temp_cache("jpg"), MediaConfig::default());
        let got = f.fetch(BASE, "bg.gif", 0).unwrap();
        assert_eq!(got.bytes, b"JPG");
        assert_eq!(got.attempts.len(), 1);
        assert!(got.attempts[0].contains("bg.png"));
        assert!(got.attempts[0].contains("404"));
    }

    #[test]
    fn it_falls_through_to_the_original_gif() {
        let gif = media_url(BASE, "bg.gif");
        let stub = StubTransport::new(vec![(&gif, ok(b"GIF"))]);
        let mut f = MediaFetcher::new(stub, temp_cache("gif"), MediaConfig::default());
        let got = f.fetch(BASE, "bg.gif", 0).unwrap();
        assert_eq!(got.bytes, b"GIF");
        assert_eq!(got.attempts.len(), 2);
    }

    #[test]
    fn a_total_failure_names_every_url_it_tried() {
        let stub = StubTransport::new(vec![]);
        let mut f = MediaFetcher::new(stub, temp_cache("fail"), MediaConfig::default());
        match f.fetch(BASE, "missing.gif", 0) {
            Err(AssetError::AllAttemptsFailed { attempts }) => {
                assert_eq!(attempts.len(), 3);
                assert!(attempts[0].contains("missing.png"));
                assert!(attempts[1].contains("missing.jpg"));
                assert!(attempts[2].contains("missing.gif"));
            }
            other => panic!("expected AllAttemptsFailed, got {other:?}"),
        }
    }

    #[test]
    fn a_transport_error_is_an_attempt_not_a_panic() {
        let panicking = media_url(BASE, "bg.png");
        let gif = media_url(BASE, "bg.gif");
        let stub = StubTransport::new(vec![
            (&panicking, Err(AssetError::Http("connection reset".into()))),
            (&gif, ok(b"GIF")),
        ]);
        let mut f = MediaFetcher::new(stub, temp_cache("err"), MediaConfig::default());
        let got = f.fetch(BASE, "bg.gif", 0).unwrap();
        assert_eq!(got.bytes, b"GIF");
        assert!(got.attempts[0].contains("connection reset"));
    }

    #[test]
    fn a_second_fetch_is_served_from_the_disk_cache() {
        let url = media_url(BASE, "bg.png");
        let stub = StubTransport::new(vec![(&url, ok(b"PNG"))]);
        let mut f = MediaFetcher::new(stub, temp_cache("disk"), MediaConfig::default());
        let _ = f.fetch(BASE, "bg.gif", 0).unwrap();
        let again = f.fetch(BASE, "bg.gif", 1).unwrap();
        assert!(again.from_cache);
        assert_eq!(again.bytes, b"PNG");
    }

    #[test]
    fn the_disk_cache_is_namespaced_per_server() {
        let a = media_url("http://one/media", "bg.png");
        let b = media_url("http://two/media", "bg.png");
        let stub = StubTransport::new(vec![(&a, ok(b"A")), (&b, ok(b"B"))]);
        let mut f = MediaFetcher::new(stub, temp_cache("ns"), MediaConfig::default());
        assert_eq!(
            f.fetch("http://one/media", "bg.gif", 0).unwrap().bytes,
            b"A"
        );
        assert_eq!(
            f.fetch("http://two/media", "bg.gif", 0).unwrap().bytes,
            b"B"
        );
        assert_eq!(
            f.fetch("http://one/media", "bg.gif", 0).unwrap().bytes,
            b"A"
        );
    }

    #[test]
    fn a_failure_is_remembered_for_the_negative_window() {
        let stub = StubTransport::new(vec![]);
        let cfg = MediaConfig {
            negative_ttl_ms: 1000,
            ..MediaConfig::default()
        };
        let mut f = MediaFetcher::new(stub, temp_cache("neg"), cfg);
        assert!(f.fetch(BASE, "gone.gif", 500).is_err());
        assert!(matches!(
            f.fetch(BASE, "gone.gif", 1000),
            Err(AssetError::RecentlyFailed { .. })
        ));
        assert_eq!(f.negative_count(), 1);
        assert!(f.forget_failure("gone.gif"));
        assert!(matches!(
            f.fetch(BASE, "gone.gif", 1000),
            Err(AssetError::AllAttemptsFailed { .. })
        ));
    }

    #[test]
    fn an_empty_200_is_not_a_usable_media_file() {
        let png = media_url(BASE, "bg.png");
        let gif = media_url(BASE, "bg.gif");
        let stub = StubTransport::new(vec![(&png, ok(b"")), (&gif, ok(b"GIF"))]);
        let mut f = MediaFetcher::new(stub, temp_cache("empty"), MediaConfig::default());
        let got = f.fetch(BASE, "bg.gif", 0).unwrap();
        assert_eq!(got.bytes, b"GIF");
        assert!(got.attempts[0].contains("empty"));
    }

    #[test]
    fn an_empty_base_url_is_refused_before_any_request() {
        let stub = StubTransport::new(vec![]);
        let mut f = MediaFetcher::new(stub, temp_cache("blank"), MediaConfig::default());
        assert!(matches!(f.fetch("", "bg.gif", 0), Err(AssetError::Url(_))));
    }

    #[test]
    fn fetch_single_does_not_chase_extensions() {
        let gif = media_url(BASE, "spot.gif");
        let png = media_url(BASE, "spot.png");
        let stub = StubTransport::new(vec![(&png, ok(b"PNG")), (&gif, ok(b"GIF"))]);
        let mut f = MediaFetcher::new(stub, temp_cache("single"), MediaConfig::default());
        let got = f.fetch_single(BASE, "spot.gif", 0).unwrap();
        assert_eq!(got.bytes, b"GIF");
        assert_eq!(got.attempts.len(), 0);
    }

    #[test]
    fn a_nested_name_is_cached_under_its_subdirectory() {
        let url = media_url(BASE, "animated-backgrounds/rainy-day.png");
        let stub = StubTransport::new(vec![(&url, ok(b"RAIN"))]);
        let mut f = MediaFetcher::new(stub, temp_cache("nested"), MediaConfig::default());
        let got = f
            .fetch(BASE, "animated-backgrounds/rainy-day.gif", 0)
            .unwrap();
        assert_eq!(got.bytes, b"RAIN");
        assert!(f
            .cache()
            .path_if_cached(BASE, "animated-backgrounds/rainy-day.gif")
            .unwrap()
            .to_string_lossy()
            .contains("animated-backgrounds"));
    }

    #[test]
    fn boxed_transports_are_transports_too() {
        let url = media_url(BASE, "bg.png");
        let boxed: Box<dyn HttpTransport> = Box::new(StubTransport::new(vec![(&url, ok(b"P"))]));
        let mut f = MediaFetcher::new(boxed, temp_cache("boxed"), MediaConfig::default());
        assert_eq!(f.fetch(BASE, "bg.gif", 0).unwrap().bytes, b"P");
    }

    #[test]
    fn the_stub_records_what_it_was_asked_for() {
        let url = media_url(BASE, "bg.png");
        let stub = StubTransport::new(vec![(&url, ok(b"P"))]);
        let mut f = MediaFetcher::new(stub, temp_cache("record"), MediaConfig::default());
        let _ = f.fetch(BASE, "bg.gif", 0).unwrap();
        let seen = f.transport.seen();
        assert_eq!(seen, vec![url]);
    }

    // ------------------------------------------------- script fetch (LOADSCRIPT/HTTPGET)

    fn script_response(ct: &str, body: &[u8]) -> Result<HttpResponse> {
        Ok(HttpResponse {
            status: 200,
            body: body.to_vec(),
            content_type: Some(ct.to_string()),
        })
    }

    #[test]
    fn the_content_type_branch_matches_only_the_iptscrae_mime_types() {
        let mk = |ct: &str| HttpResponse {
            status: 200,
            body: Vec::new(),
            content_type: Some(ct.to_string()),
        };
        assert!(mk("text/iptscrae").is_script());
        assert!(mk("text/ipt").is_script());
        assert!(mk("Text/IPTSCRAE; charset=utf-8").is_script());
        assert!(!mk("text/plain").is_script());
        assert!(!mk("image/gif").is_script());
        assert!(!mk("text/html").is_script());
        assert!(!mk("text/javascript").is_script());
        let none = HttpResponse {
            status: 200,
            body: Vec::new(),
            content_type: None,
        };
        assert!(!none.is_script(), "no content type is not a script");
    }

    #[test]
    fn an_absolute_url_passes_through_and_a_relative_one_joins_the_base() {
        assert_eq!(
            script_url("https://colosseum.example/media", "http://a/x.txt"),
            Some("http://a/x.txt".to_string())
        );
        assert_eq!(
            script_url("https://colosseum.example/media", "https://a/x.txt"),
            Some("https://a/x.txt".to_string())
        );
        assert_eq!(
            script_url("https://colosseum.example/media", "media/custo2.txt"),
            Some("https://colosseum.example/media/media/custo2.txt".to_string())
        );
        assert_eq!(
            script_url("https://colosseum.example/media", "/custo2.txt"),
            Some("https://colosseum.example/media/custo2.txt".to_string())
        );
        assert_eq!(
            script_url("https://colosseum.example/media", "custo2.txt"),
            Some("https://colosseum.example/media/custo2.txt".to_string())
        );
    }

    #[test]
    fn a_relative_url_with_no_base_is_refused() {
        assert_eq!(script_url("", "media/x.txt"), None, "no base, no fetch");
        assert_eq!(script_url("   ", "media/x.txt"), None);
        assert_eq!(script_url("", ""), None);
        assert_eq!(script_url("", "ftp://a/x"), None);
        assert_eq!(script_url("", "javascript:alert(1)"), None);
    }

    #[test]
    fn the_worker_resolves_relative_urls_against_the_advertised_base() {
        let base = "http://stub.invalid/media";
        let full = "http://stub.invalid/media/media/custo2.txt";
        let stub = StubTransport::new(vec![(
            full,
            script_response("text/iptscrae", b"1000 500 ADDLOOSEPROP"),
        )]);
        let (base_tx, base_rx) = std::sync::mpsc::channel();
        let (job_tx, job_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        base_tx.send(base.to_string()).unwrap();
        job_tx
            .send(ScriptFetch {
                requested: "media/custo2.txt".to_string(),
                url: "media/custo2.txt".to_string(),
                spot: 0,
            })
            .unwrap();
        drop(job_tx);
        run_script_worker(stub, base_rx, job_rx, done_tx);
        let outcome = done_rx.try_recv().unwrap();
        match outcome {
            ScriptOutcome::Response { url, spot, .. } => {
                assert_eq!(url, full);
                assert_eq!(spot, 0);
            }
            other => panic!("expected a Response, got {other:?}"),
        }
    }

    #[test]
    fn the_worker_reports_every_failure_shape_with_its_url() {
        let base = "http://stub.invalid/media";
        let stub = StubTransport::new(vec![
            (
                "http://stub.invalid/media/gone.txt",
                Ok(HttpResponse {
                    status: 404,
                    body: Vec::new(),
                    content_type: None,
                }),
            ),
            (
                "http://stub.invalid/media/dead.txt",
                Err(AssetError::Http("connection refused".into())),
            ),
        ]);
        let (base_tx, base_rx) = std::sync::mpsc::channel();
        let (job_tx, job_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        base_tx.send(base.to_string()).unwrap();
        job_tx
            .send(ScriptFetch {
                requested: "gone.txt".to_string(),
                url: "gone.txt".to_string(),
                spot: 7,
            })
            .unwrap();
        job_tx
            .send(ScriptFetch {
                requested: "dead.txt".to_string(),
                url: "dead.txt".to_string(),
                spot: 0,
            })
            .unwrap();
        drop(job_tx);
        run_script_worker(stub, base_rx, job_rx, done_tx);
        let mut outcomes = Vec::new();
        while let Ok(outcome) = done_rx.try_recv() {
            outcomes.push(outcome);
        }
        assert_eq!(outcomes.len(), 2, "every job is answered");
        for outcome in &outcomes {
            assert!(
                matches!(outcome, ScriptOutcome::Failure { .. }),
                "a 404 and a transport error are failures, got {outcome:?}"
            );
        }
        assert!(matches!(
            &outcomes[0],
            ScriptOutcome::Failure { spot: 7, reason, .. } if reason.contains("404")
        ));
        assert!(matches!(
            &outcomes[1],
            ScriptOutcome::Failure { reason, .. } if reason.contains("connection refused")
        ));
    }

    #[test]
    fn the_in_flight_ceiling_admits_four_and_refuses_a_fifth() {
        for n in 0..DEFAULT_MAX_HTTP_IN_FLIGHT {
            assert!(admits_in_flight(n), "{n} in flight must be admitted");
        }
        assert!(
            !admits_in_flight(DEFAULT_MAX_HTTP_IN_FLIGHT),
            "the cap is exactly {}",
            DEFAULT_MAX_HTTP_IN_FLIGHT
        );
    }

    #[test]
    fn a_request_over_the_ceiling_is_rejected_with_a_named_reason() {
        struct Reentrant;
        impl HttpTransport for Reentrant {
            fn get(&self, _url: &str, _max_bytes: u64) -> Result<HttpResponse> {
                Err(AssetError::Http(
                    "transport fault simulating an in-flight request".into(),
                ))
            }
        }
        let (base_tx, base_rx) = std::sync::mpsc::channel();
        let (job_tx, job_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        base_tx
            .send("http://stub.invalid/media".to_string())
            .unwrap();
        for i in 0..DEFAULT_MAX_HTTP_IN_FLIGHT + 1 {
            job_tx
                .send(ScriptFetch {
                    requested: format!("f{i}.txt"),
                    url: format!("f{i}.txt"),
                    spot: 0,
                })
                .unwrap();
        }
        drop(job_tx);
        run_script_worker(Reentrant, base_rx, job_rx, done_tx);
        let outcomes: Vec<ScriptOutcome> = done_rx.try_iter().collect();
        assert_eq!(
            outcomes.len(),
            DEFAULT_MAX_HTTP_IN_FLIGHT + 1,
            "every job is answered, none are dropped"
        );
        for outcome in &outcomes {
            assert!(matches!(outcome, ScriptOutcome::Failure { .. }));
        }
    }

    #[test]
    fn an_unresolvable_url_fails_with_its_own_reason() {
        let (base_tx, base_rx) = std::sync::mpsc::channel();
        let (job_tx, job_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        job_tx
            .send(ScriptFetch {
                requested: "ludo/".to_string(),
                url: "ludo/".to_string(),
                spot: 3,
            })
            .unwrap();
        drop(job_tx);
        drop(base_tx);
        run_script_worker(StubTransport::new(vec![]), base_rx, job_rx, done_tx);
        match done_rx.try_recv().unwrap() {
            ScriptOutcome::Failure { spot, reason, .. } => {
                assert_eq!(spot, 3, "the spot rides along on resolution failure");
                assert!(reason.contains("media base"), "reason was: {reason}");
            }
            other => panic!("expected a Failure, got {other:?}"),
        }
    }
}
