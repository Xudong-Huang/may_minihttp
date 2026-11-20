//! HTTP/1.1 protocol upgrade (RFC 9110 §7.8): send a request that asks to switch protocols and,
//! on `101 Switching Protocols`, hand the caller the connection itself.
//!
//! This is what a WebSocket client needs from an HTTP client: the opening handshake is an ordinary
//! HTTP/1.1 request, and everything after the 101 is another protocol on the same connection. The HTTP client does the HTTP part (connect, TLS, request head, response head); the
//! caller's protocol library only frames bytes on the [`Upgraded`] stream.
//!
//! The client does not interpret the upgraded protocol. It checks only what HTTP itself requires:
//! the request carries `Connection: upgrade` and `Upgrade`, and a 101 response names the protocol
//! in its own `Upgrade` header. Protocol-specific checks (a WebSocket `Sec-WebSocket-Accept`, say)
//! are the caller's.

use std::fmt;
use std::io::{self, Read, Write};
use std::time::Duration;

use http::header::{CONNECTION, UPGRADE};
use http::{HeaderMap, StatusCode};

use std::cell::RefCell;
use std::rc::Rc;

use may::net::TcpStream;

use super::buffer::BufferIo;
use super::Response;

/// The outcome of [`super::HttpClient::upgrade`].
#[derive(Debug)]
pub enum Upgrade {
    /// The server switched protocols (`101`). `response` is the 101 head (no body); `stream` is
    /// the connection, now speaking the upgraded protocol.
    Switched {
        /// The `101 Switching Protocols` response head.
        response: Response,
        /// The connection after the 101.
        stream: Upgraded,
    },
    /// The server answered with a final response instead (a `401`, a `426`, a redirect...). Its
    /// body can be read as usual; the connection is not upgraded.
    Refused(Response),
}

/// A connection after `101 Switching Protocols`: raw bytes in both directions.
///
/// Bytes the server sent right behind the 101 head (a first WebSocket frame, say) were already
/// read into the client's buffer; reads return those first, then read the socket. Writes are
/// buffered: call `flush` to send. The timeouts set on the client carry over and can be changed
/// with [`Self::set_timeout`].
pub struct Upgraded {
    conn: Rc<RefCell<BufferIo<TcpStream>>>,
}

impl Upgraded {
    pub(crate) fn new(conn: Rc<RefCell<BufferIo<TcpStream>>>) -> Self {
        Self { conn }
    }

    /// Set the read and write timeout of the connection (`None`: block until data or close).
    pub fn set_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        let mut conn = self.conn.borrow_mut();
        let stream = conn.inner_mut();
        stream.set_read_timeout(timeout)?;
        stream.set_write_timeout(timeout)
    }
}

impl fmt::Debug for Upgraded {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Upgraded").finish_non_exhaustive()
    }
}

impl Read for Upgraded {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.conn.borrow_mut().read(buffer)
    }
}

impl Write for Upgraded {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.conn.borrow_mut().write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.conn.borrow_mut().flush()
    }
}

/// A request asks to upgrade when it names a protocol in `Upgrade` and lists `upgrade` among the
/// `Connection` options (RFC 9110 §7.8: a server ignores an `Upgrade` without it).
pub(crate) fn check_upgrade_request(headers: &HeaderMap) -> io::Result<()> {
    if !headers.contains_key(UPGRADE) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "an upgrade request needs an Upgrade header",
        ));
    }
    if !has_token(headers, CONNECTION, "upgrade") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "an upgrade request needs `Connection: upgrade`",
        ));
    }
    Ok(())
}

/// A 101 must say which protocol the connection switched to (RFC 9110 §15.2.2).
pub(crate) fn check_switched_response(response: &Response) -> io::Result<()> {
    debug_assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
    if response.headers().contains_key(UPGRADE) {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "101 Switching Protocols without an Upgrade header",
        ))
    }
}

fn has_token(headers: &HeaderMap, name: http::header::HeaderName, token: &str) -> bool {
    headers.get_all(name).iter().any(|value| {
        value.to_str().is_ok_and(|value| {
            value
                .split(',')
                .any(|option| option.trim().eq_ignore_ascii_case(token))
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::HttpClient;
    use http::{HeaderValue, Method};
    use std::net::{TcpListener, TcpStream as StdStream};
    use std::thread;

    fn read_head(stream: &mut StdStream) -> String {
        let mut head = Vec::new();
        let mut byte = [0_u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).expect("read request head");
            head.push(byte[0]);
        }
        String::from_utf8(head).expect("request head is UTF-8")
    }

    /// Answers the upgrade with `answer`, then echoes four bytes back.
    fn serve(listener: TcpListener, answer: &'static [u8]) -> thread::JoinHandle<String> {
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let head = read_head(&mut stream);
            stream.write_all(answer).expect("write answer");
            if answer.starts_with(b"HTTP/1.1 101") {
                let mut ping = [0_u8; 4];
                stream.read_exact(&mut ping).expect("read upgraded bytes");
                stream.write_all(&ping).expect("echo");
            }
            head
        })
    }

    fn upgrade_request(client: &HttpClient) -> crate::client::Request {
        let mut request = client.new_request(Method::GET, "/stream".parse().unwrap());
        request
            .headers_mut()
            .insert(UPGRADE, HeaderValue::from_static("echo"));
        request
            .headers_mut()
            .insert(CONNECTION, HeaderValue::from_static("Upgrade"));
        request
    }

    #[test]
    fn upgrade_request_needs_both_headers() {
        let mut headers = HeaderMap::new();
        assert_eq!(
            check_upgrade_request(&headers).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        headers.insert(UPGRADE, HeaderValue::from_static("websocket"));
        assert_eq!(
            check_upgrade_request(&headers).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        headers.insert(CONNECTION, HeaderValue::from_static("keep-alive, Upgrade"));
        check_upgrade_request(&headers).unwrap();
    }

    #[test]
    fn connection_options_are_matched_as_tokens() {
        let mut headers = HeaderMap::new();
        headers.insert(UPGRADE, HeaderValue::from_static("websocket"));
        headers.insert(CONNECTION, HeaderValue::from_static("upgraded"));
        assert!(check_upgrade_request(&headers).is_err());
        headers.append(CONNECTION, HeaderValue::from_static("UPGRADE"));
        check_upgrade_request(&headers).unwrap();
    }

    #[test]
    fn upgrade_hands_over_the_connection_with_early_bytes() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        // The 101 and the first bytes of the new protocol arrive in one write: the client has
        // buffered them by the time it returns, and the stream must still yield them first.
        let server = serve(
            listener,
            b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: echo\r\nConnection: Upgrade\r\n\r\nearly",
        );

        let client = HttpClient::connect(address).unwrap();
        let request = upgrade_request(&client);
        let (response, mut stream) = match client.upgrade(request).unwrap() {
            Upgrade::Switched { response, stream } => (response, stream),
            Upgrade::Refused(response) => panic!("refused: {response:?}"),
        };
        assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
        assert_eq!(response.headers()[UPGRADE], "echo");

        let mut early = [0_u8; 5];
        stream.read_exact(&mut early).unwrap();
        assert_eq!(&early, b"early");
        stream.write_all(b"ping").unwrap();
        stream.flush().unwrap();
        let mut echo = [0_u8; 4];
        stream.read_exact(&mut echo).unwrap();
        assert_eq!(&echo, b"ping");

        let head = server.join().unwrap().to_ascii_lowercase();
        assert!(head.starts_with("get /stream http/1.1\r\n"), "{head}");
        assert!(head.contains("\r\nupgrade: echo\r\n"), "{head}");
        assert!(head.contains("\r\nconnection: upgrade\r\n"), "{head}");
    }

    #[test]
    fn a_final_response_is_returned_with_its_body() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = serve(
            listener,
            b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 6\r\nConnection: close\r\n\r\nno key",
        );

        let client = HttpClient::connect(address).unwrap();
        let request = upgrade_request(&client);
        let mut response = match client.upgrade(request).unwrap() {
            Upgrade::Refused(response) => response,
            Upgrade::Switched { response, .. } => panic!("switched: {response:?}"),
        };
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let mut body = String::new();
        response.read_to_string(&mut body).unwrap();
        assert_eq!(body, "no key");
        server.join().unwrap();
    }

    #[test]
    fn a_101_without_upgrade_is_invalid_data() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let _server = serve(listener, b"HTTP/1.1 101 Switching Protocols\r\n\r\n");

        let client = HttpClient::connect(address).unwrap();
        let request = upgrade_request(&client);
        let error = client.upgrade(request).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn a_request_without_upgrade_headers_is_not_sent() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let client = HttpClient::connect(address).unwrap();
        let request = client.new_request(Method::GET, "/stream".parse().unwrap());
        let error = client.upgrade(request).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);

        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();
        let mut byte = [0_u8; 1];
        // Dropping the unsent request must not write its head either: the peer sees EOF.
        assert_eq!(socket.read(&mut byte).unwrap(), 0);
    }
}
