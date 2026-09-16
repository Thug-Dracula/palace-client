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
}
