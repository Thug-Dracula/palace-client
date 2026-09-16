//! The blocking socket session: connect, handshake, frame in/out.
//!
//! `palace-probe` has the same logic but is a binary with no library target, so
//! the runtime reimplements the thin transport here on top of the verified
//! `palace-wire` framing. No protocol decoding lives here — only bytes.

use std::io::{ErrorKind, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use palace_wire::byteorder::ByteOrder;
use palace_wire::error::{Result as WireResult, WireError, MAX_PAYLOAD_LEN};
use palace_wire::frame::{read_handshake, Frame, Handshake, HEADER_LEN};

use crate::error::{ClientError, Result};

/// How long a single non-blocking read waits before returning empty.
pub const POLL_SLICE: Duration = Duration::from_millis(40);

/// An open connection to a pserver.
#[derive(Debug)]
pub struct Connection {
    stream: TcpStream,
    order: ByteOrder,
    buf: Vec<u8>,
    host: String,
    port: u16,
}

impl Connection {
    /// Open a TCP connection. `timeout` bounds the connect and every read.
    pub fn connect(host: &str, port: u16, timeout: Duration) -> Result<Self> {
        let addr = (host, port)
            .to_socket_addrs()
            .map_err(ClientError::Io)?
            .next()
            .ok_or_else(|| {
                ClientError::Config(format!("{host}:{port} resolved to no addresses"))
            })?;
        let stream = TcpStream::connect_timeout(&addr, timeout)
            .or_else(|_| TcpStream::connect((host, port)))
            .map_err(ClientError::Io)?;
        stream.set_nodelay(true).map_err(ClientError::Io)?;
        stream
            .set_read_timeout(Some(POLL_SLICE))
            .map_err(ClientError::Io)?;
        stream
            .set_write_timeout(Some(timeout))
            .map_err(ClientError::Io)?;
        Ok(Connection {
            stream,
            order: ByteOrder::NATIVE,
            buf: Vec::with_capacity(4096),
            host: host.to_string(),
            port,
        })
    }

    /// The host this session dialled.
    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }

    /// The port this session dialled.
    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The session byte order, fixed by the `TIYID` banner.
    #[must_use]
    pub fn order(&self) -> ByteOrder {
        self.order
    }

    /// Read the server's `TIYID` banner and fix the byte order from it.
    pub fn handshake(&mut self, timeout: Duration) -> Result<Handshake> {
        self.stream
            .set_read_timeout(Some(timeout))
            .map_err(ClientError::Io)?;
        let result = read_handshake(&mut self.stream);
        let _ = self.stream.set_read_timeout(Some(POLL_SLICE));
        let handshake = result?;
        self.order = handshake.byte_order;
        Ok(handshake)
    }

    /// Write one frame.
    pub fn send(&mut self, frame: &Frame) -> Result<()> {
        let bytes = frame.encode(self.order)?;
        self.stream.write_all(&bytes).map_err(ClientError::Io)?;
        self.stream.flush().map_err(ClientError::Io)?;
        Ok(())
    }

    fn take_frame(&mut self) -> WireResult<Option<Frame>> {
        if self.buf.len() < HEADER_LEN {
            return Ok(None);
        }
        let head: [u8; HEADER_LEN] = self
            .buf
            .get(..HEADER_LEN)
            .and_then(|s| s.try_into().ok())
            .ok_or(WireError::UnexpectedEof {
                needed: HEADER_LEN,
                available: self.buf.len(),
            })?;
        let len_bytes = [head[4], head[5], head[6], head[7]];
        let len = match self.order {
            ByteOrder::Little => u32::from_le_bytes(len_bytes),
            ByteOrder::Big => u32::from_be_bytes(len_bytes),
        };
        if len > MAX_PAYLOAD_LEN {
            return Err(WireError::PayloadTooLarge {
                length: len as usize,
            });
        }
        let total = HEADER_LEN + len as usize;
        if self.buf.len() < total {
            return Ok(None);
        }
        let frame_bytes: Vec<u8> = self.buf.drain(..total).collect();
        Ok(Some(Frame::decode_from(&frame_bytes, self.order)?))
    }

    /// Wait up to `budget` for one frame.
    ///
    /// Returns `Ok(None)` when the budget expires with nothing to read, and
    /// [`ClientError::Disconnected`] when the peer closes.
    pub fn poll_frame(&mut self, budget: Duration) -> Result<Option<Frame>> {
        let deadline = Instant::now() + budget;
        loop {
            if let Some(frame) = self.take_frame()? {
                return Ok(Some(frame));
            }
            let mut chunk = [0u8; 8192];
            match self.stream.read(&mut chunk) {
                Ok(0) => return Err(ClientError::Disconnected),
                Ok(n) => self.buf.extend_from_slice(&chunk[..n]),
                Err(e)
                    if matches!(
                        e.kind(),
                        ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
                    ) =>
                {
                    if Instant::now() >= deadline {
                        return Ok(None);
                    }
                }
                Err(e) => return Err(ClientError::Io(e)),
            }
        }
    }
}
