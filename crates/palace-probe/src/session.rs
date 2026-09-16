//! A blocking Palace session: handshake, framed send/receive, keepalive, and
//! optional capture into a [`Fixture`].

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use palace_wire::byteorder::ByteOrder;
use palace_wire::error::{Result, WireError, MAX_PAYLOAD_LEN};
use palace_wire::fixture::{Direction, Fixture};
use palace_wire::frame::{read_handshake, Frame, Handshake, HEADER_LEN};
use palace_wire::opcode::{PING, PONG};

/// Result of trying to read one frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadOutcome {
    /// A complete frame arrived.
    Frame(Frame),
    /// Nothing arrived within the timeout (the buffer may hold a partial frame).
    Timeout,
    /// The peer closed the connection.
    Closed,
}

/// A live Palace connection.
pub struct Session {
    stream: TcpStream,
    buffer: Vec<u8>,
    order: ByteOrder,
    auto_pong: bool,
    fixture: Option<Fixture>,
    closed: bool,
}

impl Session {
    /// Open a TCP connection and set a coarse read timeout. Per-frame timeouts
    /// are applied by [`Session::next_frame`].
    pub fn connect(host: &str, port: u16) -> std::io::Result<Session> {
        let stream = TcpStream::connect((host, port))?;
        stream.set_nodelay(true)?;
        Ok(Session {
            stream,
            buffer: Vec::with_capacity(16 * 1024),
            order: ByteOrder::NATIVE,
            auto_pong: true,
            fixture: None,
            closed: false,
        })
    }

    /// The capture, if one is being recorded.
    pub fn fixture(&self) -> Option<&Fixture> {
        self.fixture.as_ref()
    }

    /// Start recording every frame into a fixture, and record the already-read
    /// banner frame.
    pub fn start_capture(&mut self, server: impl Into<String>, banner: Frame) {
        let mut fx = Fixture::new(server, self.order);
        fx.push(Direction::Server, banner);
        self.fixture = Some(fx);
    }

    /// Read the server banner and fix the session byte order.
    pub fn handshake(&mut self) -> Result<Handshake> {
        let hs = read_handshake(&mut self.stream)?;
        self.order = hs.byte_order;
        Ok(hs)
    }

    /// Encode and send a frame, recording it when capturing.
    pub fn send(&mut self, frame: Frame) -> Result<()> {
        let bytes = frame.encode(self.order)?;
        self.stream.write_all(&bytes)?;
        self.stream.flush()?;
        if let Some(fx) = self.fixture.as_mut() {
            fx.push(Direction::Client, frame);
        }
        Ok(())
    }

    /// Read one frame, waiting at most `timeout`.
    ///
    /// Incoming `ping` messages are answered with `pong` automatically (both
    /// get recorded), so the caller only sees the payload traffic it cares
    /// about plus the keepalives.
    pub fn next_frame(&mut self, timeout: Duration) -> Result<ReadOutcome> {
        if self.closed {
            return Ok(ReadOutcome::Closed);
        }
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(frame) = self.try_decode()? {
                if self.auto_pong && frame.opcode == PING {
                    self.send(Frame::empty(PONG, frame.ref_num))?;
                }
                return Ok(ReadOutcome::Frame(frame));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(ReadOutcome::Timeout);
            }
            let slice = remaining.min(Duration::from_millis(250));
            self.stream.set_read_timeout(Some(slice))?;
            let mut chunk = [0u8; 8192];
            match self.stream.read(&mut chunk) {
                Ok(0) => {
                    self.closed = true;
                    return Ok(ReadOutcome::Closed);
                }
                Ok(n) => self.buffer.extend_from_slice(&chunk[..n]),
                Err(e)
                    if e.kind() == std::io::ErrorKind::TimedOut
                        || e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(WireError::Io(e)),
            }
        }
    }

    /// Read frames until the connection goes quiet for `quiet`, or `hard`
    /// elapses in total.
    pub fn drain(&mut self, quiet: Duration, hard: Duration) -> Result<Vec<Frame>> {
        let started = Instant::now();
        let mut last_frame = Instant::now();
        let mut frames = Vec::new();
        while started.elapsed() < hard {
            match self.next_frame(Duration::from_millis(400))? {
                ReadOutcome::Frame(frame) => {
                    last_frame = Instant::now();
                    frames.push(frame);
                }
                ReadOutcome::Closed => break,
                ReadOutcome::Timeout => {
                    if last_frame.elapsed() >= quiet {
                        break;
                    }
                }
            }
        }
        Ok(frames)
    }

    /// Drain until `predicate` matches a frame (which is returned) or `hard`
    /// elapses.
    pub fn wait_for<F>(&mut self, hard: Duration, mut predicate: F) -> Result<Option<Frame>>
    where
        F: FnMut(&Frame) -> bool,
    {
        let started = Instant::now();
        while started.elapsed() < hard {
            match self.next_frame(Duration::from_millis(400))? {
                ReadOutcome::Frame(frame) => {
                    if predicate(&frame) {
                        return Ok(Some(frame));
                    }
                }
                ReadOutcome::Closed => break,
                ReadOutcome::Timeout => {}
            }
        }
        Ok(None)
    }

    fn try_decode(&mut self) -> Result<Option<Frame>> {
        if self.buffer.len() < HEADER_LEN {
            return Ok(None);
        }
        let mut header = palace_wire::Reader::new(&self.buffer, self.order);
        let _opcode = header.read_u32()?;
        let length = header.read_u32()?;
        if length > MAX_PAYLOAD_LEN {
            return Err(WireError::ImplausibleLength {
                length,
                max: MAX_PAYLOAD_LEN,
            });
        }
        let total = HEADER_LEN + length as usize;
        if self.buffer.len() < total {
            return Ok(None);
        }
        let frame = Frame::decode_from(&self.buffer[..total], self.order)?;
        self.buffer.drain(..total);
        if let Some(fx) = self.fixture.as_mut() {
            fx.push(Direction::Server, frame.clone());
        }
        Ok(Some(frame))
    }
}
