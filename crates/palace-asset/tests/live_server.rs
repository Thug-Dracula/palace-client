//! Opt-in test against a live pserver.
//!
//! Ignored by default: it needs `localhost:9998` reachable and it performs a
//! real logon. Run it with
//!
//! ```sh
//! cargo test -p palace-asset --test live_server -- --ignored --nocapture
//! ```
//!
//! The default suite never touches the network, per the project's hermetic-CI
//! rule; this file exists so the `qAst` send path and the `sAst` receive path
//! can be shaken against a real server on demand. The hard assertions are the
//! ones that hold for any healthy pserver — the `TIYID` banner, the logon burst,
//! the `room` description and the `HTTP` media-server message. The `sAst`
//! exchange is best-effort, because a server only answers for assets it holds.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use palace_asset::{AssetKey, AssetPipeline, AssetType, PipelineEvent};
use palace_wire::byteorder::ByteOrder;
use palace_wire::frame::{read_handshake, Frame, HEADER_LEN};
use palace_wire::messages::reference_logon_record;
use palace_wire::opcode::{ASSETSEND, LOGOFF, LOGON, ROOMDESC, TIYID};
use palace_wire::{Reader, Writer};

const HOST: &str = "localhost";
const PORT: u16 = 9998;
const READ_TIMEOUT: Duration = Duration::from_millis(200);

struct Conn {
    stream: TcpStream,
    buffer: Vec<u8>,
    order: ByteOrder,
}

impl Conn {
    fn connect() -> std::io::Result<Conn> {
        let stream = TcpStream::connect((HOST, PORT))?;
        stream.set_nodelay(true)?;
        Ok(Conn {
            stream,
            buffer: Vec::new(),
            order: ByteOrder::Little,
        })
    }

    fn handshake(&mut self) -> palace_wire::Result<Frame> {
        let hs = read_handshake(&mut self.stream)?;
        self.order = hs.byte_order;
        Ok(hs.frame)
    }

    fn send(&mut self, frame: Frame) -> std::io::Result<()> {
        let bytes = frame
            .encode(self.order)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        self.stream.write_all(&bytes)?;
        self.stream.flush()
    }

    /// Read one frame, waiting at most [`READ_TIMEOUT`].
    fn read_frame(&mut self) -> Option<Frame> {
        loop {
            if let Some(frame) = self.decode_buffered() {
                return Some(frame);
            }
            self.stream.set_read_timeout(Some(READ_TIMEOUT)).ok()?;
            let mut chunk = [0u8; 8192];
            match self.stream.read(&mut chunk) {
                Ok(0) => return None,
                Ok(n) => self.buffer.extend_from_slice(&chunk[..n]),
                Err(ref e)
                    if e.kind() == std::io::ErrorKind::TimedOut
                        || e.kind() == std::io::ErrorKind::WouldBlock =>
                {
                    return None;
                }
                Err(_) => return None,
            }
        }
    }

    fn decode_buffered(&mut self) -> Option<Frame> {
        if self.buffer.len() < HEADER_LEN {
            return None;
        }
        let mut probe = Reader::new(&self.buffer, self.order);
        let _opcode = probe.read_u32().ok()?;
        let len = probe.read_u32().ok()? as usize;
        if self.buffer.len() < HEADER_LEN + len {
            return None;
        }
        let mut reader = Reader::new(&self.buffer, self.order);
        let frame = Frame::decode(&mut reader).ok()?;
        self.buffer.drain(..HEADER_LEN + len);
        Some(frame)
    }

    /// Read frames until `quiet` elapses with nothing arriving, or `hard` total.
    fn drain(&mut self, quiet: Duration, hard: Duration) -> Vec<Frame> {
        let started = Instant::now();
        let mut last = Instant::now();
        let mut out = Vec::new();
        while started.elapsed() < hard {
            match self.read_frame() {
                Some(frame) => {
                    last = Instant::now();
                    out.push(frame);
                }
                None => {
                    if last.elapsed() >= quiet {
                        break;
                    }
                }
            }
        }
        out
    }
}

#[test]
#[ignore = "needs a live pserver at localhost:9998"]
fn a_live_logon_yields_the_media_server_and_can_request_an_asset() {
    let mut conn = Conn::connect().expect("connect to the live server");
    let banner = conn.handshake().expect("handshake");
    assert_eq!(
        banner.opcode,
        TIYID,
        "the server must open with MSG_TIYID, got {}",
        banner.opcode.describe()
    );
    assert_ne!(banner.ref_num, 0, "the banner carries our assigned user id");

    let record = reference_logon_record("RustAssetProbe", 0);
    let mut w = Writer::new(conn.order);
    record.encode(&mut w);
    conn.send(Frame::new(LOGON, 0, w.into_vec()))
        .expect("send regi");

    let mut pipeline = AssetPipeline::new();
    pipeline.set_byte_order(conn.order);
    pipeline.set_query_ref_num(banner.ref_num);

    let burst = conn.drain(Duration::from_millis(600), Duration::from_secs(8));
    assert!(!burst.is_empty(), "the logon burst must arrive");
    let mut saw_room = false;
    for (index, frame) in burst.iter().enumerate() {
        if frame.opcode == ROOMDESC {
            saw_room = true;
        }
        let _ = pipeline.on_frame(frame, conn.order, index as u64);
    }
    assert!(saw_room, "the logon burst includes the initial room");
    let media = pipeline
        .media_server()
        .expect("the server advertises its media server over the HTTP opcode")
        .to_string();
    println!("live media server: {media}");

    // Best-effort: ask for a prop id the captured corpus contains and see
    // whether this server happens to hold it.
    let wanted = AssetKey::new(AssetType::PROP, 1020559380, 0);
    assert!(matches!(
        pipeline.request(wanted, 0),
        palace_asset::RequestDisposition::Queued
    ));

    let mut now = 0u64;
    let mut assembled = 0usize;
    for _ in 0..40 {
        now += 250;
        for event in pipeline.poll(now) {
            if let PipelineEvent::Send { frames } = event {
                for frame in frames {
                    conn.send(frame).expect("send qAst");
                }
            }
        }
        for frame in conn.drain(Duration::from_millis(150), Duration::from_millis(400)) {
            if frame.opcode != ASSETSEND {
                continue;
            }
            for event in pipeline.on_frame(&frame, conn.order, now) {
                if let PipelineEvent::AssetReady { key, len, .. } = event {
                    println!("assembled {key} ({len} bytes)");
                    let stored = pipeline.cache().peek(&key).expect("cached");
                    if key.crc != 0 {
                        assert_eq!(
                            stored.computed_crc(),
                            Some(key.crc),
                            "an assembled prop must satisfy the payload CRC"
                        );
                    }
                    assembled += 1;
                }
            }
        }
        if assembled > 0 {
            break;
        }
    }
    println!("live sAst assets assembled: {assembled} (the server only serves what it holds)");

    let _ = conn.send(Frame::empty(LOGOFF, 0));
}
