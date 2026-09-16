//! The media path, against a real HTTP server started by the test.
//!
//! Nothing here touches the network outside loopback, so the suite stays
//! hermetic; but the transport under test is the real [`UreqTransport`], not a
//! mock, so the request formation, status handling, body reading and the
//! byte-limit path are all genuinely exercised.
//!
//! `TestServer` is deliberately tiny — a `TcpListener` on an ephemeral port that
//! answers from a route table, records what it was asked for, and can be told to
//! stall so the timeout path can be tested without waiting on a real timeout.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use palace_asset::{
    fallback_chain, media_url, AssetError, HttpTransport, MediaCache, MediaConfig, MediaFetcher,
    UreqTransport,
};

#[derive(Clone)]
enum Reply {
    Body(Vec<u8>),
    Status(u16),
    Stall,
}

struct TestServer {
    addr: SocketAddr,
    hits: Arc<Mutex<Vec<String>>>,
    connections: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl TestServer {
    fn start(routes: Vec<(&str, Reply)>) -> TestServer {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let table: HashMap<String, Reply> = routes
            .into_iter()
            .map(|(path, reply)| (path.to_string(), reply))
            .collect();
        let hits = Arc::new(Mutex::new(Vec::new()));
        let connections = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));

        let thread_hits = Arc::clone(&hits);
        let thread_conns = Arc::clone(&connections);
        let thread_stop = Arc::clone(&stop);

        let handle = thread::spawn(move || {
            listener
                .set_nonblocking(true)
                .expect("non-blocking listener");
            while !thread_stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        thread_conns.fetch_add(1, Ordering::Relaxed);
                        let _ = stream.set_nonblocking(false);
                        serve(stream, &table, &thread_hits, &thread_stop);
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });

        TestServer {
            addr,
            hits,
            connections,
            stop,
            handle: Some(handle),
        }
    }

    fn base_url(&self) -> String {
        format!("http://{}/media", self.addr)
    }

    fn hits(&self) -> Vec<String> {
        self.hits.lock().expect("hits lock").clone()
    }

    fn connections(&self) -> usize {
        self.connections.load(Ordering::Relaxed)
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn serve(
    mut stream: TcpStream,
    table: &HashMap<String, Reply>,
    hits: &Mutex<Vec<String>>,
    stop: &AtomicBool,
) {
    let mut buf = [0u8; 4096];
    let mut read = 0usize;
    loop {
        match stream.read(&mut buf[read..]) {
            Ok(0) => return,
            Ok(n) => {
                read += n;
                if buf[..read].windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
                if read == buf.len() {
                    break;
                }
            }
            Err(_) => return,
        }
    }
    let request = String::from_utf8_lossy(&buf[..read]).to_string();
    let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();
    hits.lock().expect("hits lock").push(path.clone());

    let reply = table.get(&path).cloned().unwrap_or(Reply::Status(404));
    match reply {
        Reply::Stall => {
            // Accept and say nothing at all. Return as soon as the client gives
            // up (its read returns 0) or the test signals shutdown, so the suite
            // never waits on a sleep.
            let _ = stream.set_read_timeout(Some(Duration::from_millis(50)));
            let mut sink = [0u8; 256];
            let deadline = Instant::now() + Duration::from_secs(10);
            while Instant::now() < deadline && !stop.load(Ordering::Relaxed) {
                match stream.read(&mut sink) {
                    Ok(0) => break,
                    Ok(_) => continue,
                    Err(ref e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            || e.kind() == std::io::ErrorKind::TimedOut => {}
                    Err(_) => break,
                }
            }
        }
        Reply::Status(status) => {
            let _ = stream.write_all(
                format!(
                    "HTTP/1.1 {status} Whatever\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                )
                .as_bytes(),
            );
        }
        Reply::Body(body) => {
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(&body);
        }
    }
    let _ = stream.flush();
    let _ = stream.shutdown(std::net::Shutdown::Write);
}

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/media")
}

fn cache_root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("palace-asset-http-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn fast_config() -> MediaConfig {
    MediaConfig {
        connect_timeout_ms: 2_000,
        total_timeout_ms: 2_000,
        ..MediaConfig::default()
    }
}

#[test]
fn a_gif_background_is_fetched_as_png_when_the_server_has_one() {
    let png = std::fs::read(fixtures().join("bg.png")).unwrap();
    let server = TestServer::start(vec![
        ("/media/bg.png", Reply::Body(png.clone())),
        ("/media/bg.jpg", Reply::Status(404)),
        ("/media/bg.gif", Reply::Status(404)),
    ]);
    let mut fetcher = MediaFetcher::new(
        UreqTransport::from_config(&fast_config()),
        MediaCache::new(cache_root("png")),
        fast_config(),
    );

    let got = fetcher
        .fetch(&server.base_url(), "bg.gif", 0)
        .expect("the png attempt should succeed");
    assert_eq!(got.bytes, png);
    assert!(!got.from_cache);
    assert!(got.url.ends_with("/media/bg.png"));
    assert!(got.attempts.is_empty(), "the first candidate won");
    assert_eq!(server.hits(), vec!["/media/bg.png"]);
}

#[test]
fn the_chain_falls_through_png_to_jpg_to_the_original_gif() {
    let jpg = std::fs::read(fixtures().join("bg.jpg")).unwrap();
    let server = TestServer::start(vec![
        ("/media/bg.png", Reply::Status(404)),
        ("/media/bg.jpg", Reply::Body(jpg.clone())),
        ("/media/bg.gif", Reply::Status(404)),
    ]);
    let mut fetcher = MediaFetcher::new(
        UreqTransport::from_config(&fast_config()),
        MediaCache::new(cache_root("jpg")),
        fast_config(),
    );
    let got = fetcher.fetch(&server.base_url(), "bg.gif", 0).unwrap();
    assert_eq!(got.bytes, jpg);
    assert_eq!(
        server.hits(),
        vec!["/media/bg.png", "/media/bg.jpg"],
        "the original gif is never asked for once jpg answers"
    );
    assert_eq!(got.attempts.len(), 1);

    let gif = std::fs::read(fixtures().join("bg.gif")).unwrap();
    let server2 = TestServer::start(vec![
        ("/media/bg.png", Reply::Status(404)),
        ("/media/bg.jpg", Reply::Status(404)),
        ("/media/bg.gif", Reply::Body(gif.clone())),
    ]);
    let mut fetcher2 = MediaFetcher::new(
        UreqTransport::from_config(&fast_config()),
        MediaCache::new(cache_root("gif")),
        fast_config(),
    );
    let got2 = fetcher2.fetch(&server2.base_url(), "bg.gif", 0).unwrap();
    assert_eq!(got2.bytes, gif);
    assert_eq!(
        server2.hits(),
        vec!["/media/bg.png", "/media/bg.jpg", "/media/bg.gif"]
    );
    assert_eq!(got2.attempts.len(), 2);
}

#[test]
fn every_candidate_failing_says_exactly_what_was_tried() {
    let server = TestServer::start(vec![
        ("/media/missing.png", Reply::Status(404)),
        ("/media/missing.jpg", Reply::Status(500)),
        ("/media/missing.gif", Reply::Status(403)),
    ]);
    let mut fetcher = MediaFetcher::new(
        UreqTransport::from_config(&fast_config()),
        MediaCache::new(cache_root("miss")),
        fast_config(),
    );
    match fetcher.fetch(&server.base_url(), "missing.gif", 0) {
        Err(AssetError::AllAttemptsFailed { attempts }) => {
            assert_eq!(attempts.len(), 3);
            assert!(attempts[0].contains("missing.png") && attempts[0].contains("404"));
            assert!(attempts[1].contains("missing.jpg") && attempts[1].contains("500"));
            assert!(attempts[2].contains("missing.gif") && attempts[2].contains("403"));
        }
        other => panic!("expected AllAttemptsFailed, got {other:?}"),
    }
    // The failure is remembered, so a room change does not re-hammer the server.
    assert_eq!(fetcher.negative_count(), 1);
    let before = server.connections();
    assert!(matches!(
        fetcher.fetch(&server.base_url(), "missing.gif", 100),
        Err(AssetError::RecentlyFailed { .. })
    ));
    assert_eq!(server.connections(), before, "no second round of requests");
}

#[test]
fn the_disk_cache_prevents_a_second_round_trip() {
    let png = std::fs::read(fixtures().join("bg.png")).unwrap();
    let server = TestServer::start(vec![("/media/bg.png", Reply::Body(png.clone()))]);
    let root = cache_root("disk");
    let mut fetcher = MediaFetcher::new(
        UreqTransport::from_config(&fast_config()),
        MediaCache::new(&root),
        fast_config(),
    );

    let first = fetcher.fetch(&server.base_url(), "bg.gif", 0).unwrap();
    assert!(!first.from_cache);
    let hits_after_first = server.hits().len();

    let second = fetcher.fetch(&server.base_url(), "bg.gif", 1).unwrap();
    assert!(second.from_cache);
    assert_eq!(second.bytes, png);
    assert_eq!(server.hits().len(), hits_after_first, "served from disk");

    // The cached file lives under a per-server namespace and keeps its name.
    let path = fetcher
        .cache()
        .path_if_cached(&server.base_url(), "bg.gif")
        .expect("cached");
    assert!(path.starts_with(root.join("media")));
    assert!(path.to_string_lossy().ends_with("bg.gif"));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_nested_background_path_is_cached_under_its_subdirectory() {
    let body = std::fs::read(fixtures().join("avatar-editor/photopea.png")).unwrap();
    let server = TestServer::start(vec![(
        "/media/animated-backgrounds/rainy-day.png",
        Reply::Body(body.clone()),
    )]);
    let root = cache_root("nested");
    let mut fetcher = MediaFetcher::new(
        UreqTransport::from_config(&fast_config()),
        MediaCache::new(&root),
        fast_config(),
    );
    let got = fetcher
        .fetch(&server.base_url(), "animated-backgrounds/rainy-day.gif", 0)
        .unwrap();
    assert_eq!(got.bytes, body);
    let path = fetcher
        .cache()
        .path_if_cached(&server.base_url(), "animated-backgrounds/rainy-day.gif")
        .unwrap();
    assert!(path.ends_with("animated-backgrounds/rainy-day.gif"));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_server_that_never_answers_times_out_instead_of_hanging() {
    let server = TestServer::start(vec![("/media/bg.png", Reply::Stall)]);
    let cfg = MediaConfig {
        connect_timeout_ms: 150,
        total_timeout_ms: 500,
        ..MediaConfig::default()
    };
    let mut fetcher = MediaFetcher::new(
        UreqTransport::from_config(&cfg),
        MediaCache::new(cache_root("stall")),
        cfg,
    );

    let started = Instant::now();
    let outcome = fetcher.fetch(&server.base_url(), "bg.gif", 0);
    let elapsed = started.elapsed();

    assert!(
        outcome.is_err(),
        "a stalled server must not be reported as success"
    );
    assert!(
        elapsed < Duration::from_secs(10),
        "the timeout must fire, not the test harness; took {elapsed:?}"
    );
}

#[test]
fn a_connection_refused_port_is_an_error_not_a_panic() {
    // Bind and immediately drop to obtain a port nobody listens on.
    let port = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let cfg = MediaConfig {
        connect_timeout_ms: 500,
        total_timeout_ms: 1000,
        ..MediaConfig::default()
    };
    let mut fetcher = MediaFetcher::new(
        UreqTransport::from_config(&cfg),
        MediaCache::new(cache_root("refused")),
        cfg,
    );
    let base = format!("http://127.0.0.1:{port}/media");
    match fetcher.fetch(&base, "bg.gif", 0) {
        Err(AssetError::AllAttemptsFailed { attempts }) => assert_eq!(attempts.len(), 3),
        other => panic!("expected AllAttemptsFailed, got {other:?}"),
    }
}

#[test]
fn a_response_larger_than_the_limit_is_refused() {
    let png = std::fs::read(fixtures().join("bg.png")).unwrap();
    assert!(png.len() > 1000);
    let server = TestServer::start(vec![("/media/bg.png", Reply::Body(png))]);
    let cfg = MediaConfig {
        max_bytes: 100,
        connect_timeout_ms: 2_000,
        total_timeout_ms: 2_000,
        ..MediaConfig::default()
    };
    let transport = UreqTransport::from_config(&cfg);
    let err = transport
        .get(&media_url(&server.base_url(), "bg.png"), cfg.max_bytes)
        .unwrap_err();
    assert!(
        matches!(err, AssetError::BodyTooLarge { .. }),
        "expected a size refusal, got {err:?}"
    );
}

#[test]
fn the_transport_reports_status_codes_rather_than_erroring_on_them() {
    let server = TestServer::start(vec![
        ("/media/ok", Reply::Body(b"hai".to_vec())),
        ("/media/gone", Reply::Status(404)),
    ]);
    let transport = UreqTransport::from_config(&fast_config());

    let ok = transport
        .get(&media_url(&server.base_url(), "ok"), 1024)
        .unwrap();
    assert!(ok.is_success());
    assert_eq!(ok.body, b"hai");

    let gone = transport
        .get(&media_url(&server.base_url(), "gone"), 1024)
        .unwrap();
    assert_eq!(gone.status, 404);
    assert!(!gone.is_success());
}

#[test]
fn a_hotspot_image_is_fetched_by_its_exact_name() {
    // Hotspot images get no extension chain in either reference client, so
    // exactly one URL is requested even for a `.gif` name.
    let body = std::fs::read(fixtures().join("bg.gif")).unwrap();
    let server = TestServer::start(vec![("/media/notebar.gif", Reply::Body(body.clone()))]);
    let mut fetcher = MediaFetcher::new(
        UreqTransport::from_config(&fast_config()),
        MediaCache::new(cache_root("single")),
        fast_config(),
    );
    let got = fetcher
        .fetch_single(&server.base_url(), "notebar.gif", 0)
        .unwrap();
    assert_eq!(got.bytes, body);
    assert_eq!(server.hits(), vec!["/media/notebar.gif"]);
    assert_eq!(fallback_chain("notebar.gif").len(), 3);
}
