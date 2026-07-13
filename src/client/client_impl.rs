use std::cell::RefCell;
use std::io;
use std::net::ToSocketAddrs;
use std::rc::Rc;
use std::time::Duration;

use bytes::Buf;
use http::{Method, Uri};
use may::net::TcpStream;

use crate::client::buffer::BufferIo;
use crate::client::{Request, Response};

/// Coroutine HTTP/1.1 client — API-compatible with `may_http::client::HttpClient`.
#[derive(Debug)]
pub struct HttpClient {
    conn: Rc<RefCell<BufferIo<TcpStream>>>,
    expect_body: bool,
}

/// On Windows, `may::net::TcpStream::connect` can return various
/// WSA error codes for connection failures, and `raw_os_error()` may
/// be `None` when the error passes through the coroutine context.
/// Remap common connection-refusal errors so the client API reports
/// `ErrorKind::ConnectionRefused` consistently.
#[cfg(windows)]
fn connect_remap(e: io::Error) -> io::Error {
    match e.raw_os_error() {
        // WSAECONNREFUSED (10061) — connection refused
        // WSAETIMEDOUT (10060) — connection timed out (no response)
        // WSAEHOSTUNREACH (10064) — host unreachable
        Some(10061) | Some(10060) | Some(10064) => {
            io::Error::new(io::ErrorKind::ConnectionRefused, "connection refused")
        }
        _ => {
            // raw_os_error() may return None when errors pass through
            // the coroutine context; fall back to string matching
            let desc = e.to_string().to_lowercase();
            if desc.contains("refused")
                || desc.contains("timed out")
                || desc.contains("unreachable")
                || desc.contains("wsaeconnrefused")
                || desc.contains("wsaetimedout")
                || desc.contains("wsaehostunreach")
            {
                io::Error::new(io::ErrorKind::ConnectionRefused, "connection refused")
            } else {
                e
            }
        }
    }
}

impl HttpClient {
    /// Connect to the given address.
    pub fn connect<A: ToSocketAddrs>(remote: A) -> io::Result<Self> {
        #[cfg(windows)]
        let stream = TcpStream::connect(remote).map_err(connect_remap)?;
        #[cfg(not(windows))]
        let stream = TcpStream::connect(remote)?;
        let stream = BufferIo::new(stream);
        Ok(HttpClient {
            conn: Rc::new(RefCell::new(stream)),
            expect_body: true,
        })
    }

    /// Set read/write timeout on the underlying connection.
    pub fn set_timeout(&mut self, timeout: Option<Duration>) -> &mut Self {
        {
            let mut s = self.conn.borrow_mut();
            let s = s.inner_mut();
            // may::net::TcpStream timeout errors are handled at the coroutine
            // level (may::io::Timeout). The underlying socket call may return
            // EOPNOTSUPP on non-blocking sockets — this is expected.
            let _ = s.set_read_timeout(timeout);
            let _ = s.set_write_timeout(timeout);
        }
        self
    }

    /// GET shortcut — sends request on drop and reads the response.
    pub fn get(&mut self, uri: Uri) -> io::Result<Response> {
        self.expect_body = true; // GET can have a body
        let mut req = Request::new(self.conn.clone());
        *req.uri_mut() = uri;
        drop(req);
        self.get_rsp()
    }

    /// POST shortcut with body bytes.
    pub fn post<T: Buf>(&mut self, uri: Uri, mut data: T) -> io::Result<Response> {
        self.expect_body = true; // POST can have a body
        let mut req = Request::new(self.conn.clone());
        *req.method_mut() = Method::POST;
        *req.uri_mut() = uri;
        let body = data.copy_to_bytes(data.remaining());
        req.send(&body)?;
        drop(req);
        self.get_rsp()
    }

    /// Build a request with the given method and URI.
    #[inline]
    pub fn new_request(&self, method: Method, uri: Uri) -> Request {
        let mut req = Request::new(self.conn.clone());
        // HEAD requests expect no body
        if method == Method::HEAD {
            req.expect_body(false);
        }
        *req.method_mut() = method;
        *req.uri_mut() = uri;
        req
    }

    /// Send a request built from this client and read the response.
    #[inline]
    pub fn send_request(&mut self, req: Request) -> io::Result<Response> {
        use std::io::Write;
        let conn: Rc<RefCell<dyn Write>> = self.conn.clone();
        debug_assert!(
            Rc::ptr_eq(&conn, req.conn()),
            "client and request must share the same connection Rc"
        );
        self.expect_body = req.expect_body_request();
        drop(req);
        self.get_rsp()
    }

    #[inline]
    fn get_rsp(&mut self) -> io::Result<Response> {
        let mut stream = self.conn.borrow_mut();
        loop {
            match super::response::decode(stream.get_reader_buf())? {
                None => {
                    if stream.bump_read()? == 0 {
                        return Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "connection breaked",
                        ));
                    }
                }
                Some(mut rsp) => {
                    rsp.set_reader(self.conn.clone(), self.expect_body)?;
                    return Ok(rsp);
                }
            }
        }
    }
}
