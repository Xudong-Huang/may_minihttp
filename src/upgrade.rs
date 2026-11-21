//! Server-side HTTP/1.1 protocol upgrade (RFC 9110 §7.8): answer a request with
//! `101 Switching Protocols` and hand the connection to the service.
//!
//! A WebSocket server needs exactly this from an HTTP server: the opening handshake is an ordinary
//! request, and everything after the 101 is another protocol on the same connection. The service
//! decides inside [`HttpService::call`](crate::HttpService::call) by calling
//! [`Response::upgrade`](crate::Response::upgrade) with a closure. The server loop then writes the
//! responses still queued for earlier pipelined requests and the 101, in order, stops parsing HTTP
//! on the connection, and runs the closure with an [`Upgraded`] connection. When the closure
//! returns, the connection is closed.
//!
//! The server does not interpret the new protocol: protocol-specific checks (a WebSocket
//! `Sec-WebSocket-Key` and its `Sec-WebSocket-Accept`) and framing are the caller's.

use std::fmt;
use std::io::{self, Read, Write};
use std::net::SocketAddr;
use std::time::Duration;

use bytes::{Buf, BytesMut};
use may::net::TcpStream;

/// What [`Response::upgrade`](crate::Response::upgrade) runs once the 101 is on the wire.
pub(crate) type OnUpgrade = Box<dyn FnOnce(Upgraded<'_>) -> io::Result<()>>;

/// A connection after `101 Switching Protocols`: raw bytes in both directions.
///
/// Bytes the client sent right behind the upgrade request (a first WebSocket frame, say) were
/// already read into the server's buffer; reads return those first, then read the socket. Reads
/// and writes park the coroutine, not the worker thread. The connection belongs to this value for
/// as long as the upgrade closure runs; it is closed when the closure returns.
pub struct Upgraded<'c> {
    stream: &'c mut TcpStream,
    buffered: BytesMut,
}

impl<'c> Upgraded<'c> {
    pub(crate) fn new(stream: &'c mut TcpStream, buffered: BytesMut) -> Self {
        Upgraded { stream, buffered }
    }

    /// The client's address.
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.stream.peer_addr()
    }

    /// Bound each read (`None`: wait until data or close). A read that times out returns
    /// `TimedOut` or `WouldBlock`, and the connection stays usable.
    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.stream.set_read_timeout(timeout)
    }

    /// Bound each write (`None`: wait until the socket takes the data).
    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.stream.set_write_timeout(timeout)
    }

    /// Bytes received behind the upgrade request and not read yet.
    pub fn buffered(&self) -> &[u8] {
        &self.buffered
    }
}

impl fmt::Debug for Upgraded<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Upgraded")
            .field("peer", &self.stream.peer_addr().ok())
            .field("buffered", &self.buffered.len())
            .finish()
    }
}

impl Read for Upgraded<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.buffered.is_empty() {
            return self.stream.read(buffer);
        }
        let n = buffer.len().min(self.buffered.len());
        buffer[..n].copy_from_slice(&self.buffered[..n]);
        self.buffered.advance(n);
        Ok(n)
    }
}

impl Write for Upgraded<'_> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.stream.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}
