//! A real WebSocket on `Response::upgrade`: the service checks the opening handshake, answers 101
//! with `Sec-WebSocket-Accept`, and tungstenite frames messages on the upgraded connection
//! (`Role::Server`, no HTTP of its own). The client is tungstenite's handshake over a plain socket.

use may_minihttp::{HttpServer, HttpService, Request, Response};
use std::io;
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};
use tungstenite::handshake::derive_accept_key;
use tungstenite::protocol::Role;
use tungstenite::{Message, WebSocket};

#[derive(Clone)]
struct Echo;

fn header<'r>(req: &'r Request, name: &str) -> Option<&'r [u8]> {
    req.headers()
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case(name))
        .map(|h| h.value)
}

impl HttpService for Echo {
    fn call(&mut self, req: Request, rsp: &mut Response) -> io::Result<()> {
        let is_websocket = header(&req, "upgrade")
            .is_some_and(|v| v.eq_ignore_ascii_case(b"websocket"))
            && header(&req, "sec-websocket-version") == Some(b"13".as_slice());
        let Some(key) = header(&req, "sec-websocket-key").filter(|_| is_websocket) else {
            rsp.status_code(426, "Upgrade Required");
            rsp.header("Upgrade: websocket");
            return Ok(());
        };
        rsp.header(format!("Sec-WebSocket-Accept: {}", derive_accept_key(key)));
        rsp.upgrade("websocket", |conn| {
            let mut ws = WebSocket::from_raw_socket(conn, Role::Server, None);
            loop {
                match ws.read() {
                    Ok(Message::Text(t)) => ws
                        .send(Message::text(format!("echo: {t}")))
                        .map_err(io::Error::other)?,
                    Ok(Message::Binary(b)) => {
                        ws.send(Message::Binary(b)).map_err(io::Error::other)?
                    }
                    // tungstenite queues the reply to a Close and sends it on the next read; the
                    // session ends when that read reports the connection closed.
                    Err(tungstenite::Error::ConnectionClosed) => return Ok(()),
                    Ok(_) => {}
                    Err(e) => return Err(io::Error::other(e)),
                }
            }
        });
        Ok(())
    }
}

fn server() -> u16 {
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let _handle = HttpServer(Echo).start(("127.0.0.1", port)).unwrap();
    let started = Instant::now();
    while TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "server did not start"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    port
}

#[test]
fn websocket_messages_round_trip_over_the_upgraded_connection() {
    let port = server();
    let socket = TcpStream::connect(("127.0.0.1", port)).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let (mut ws, response) =
        tungstenite::client::client(format!("ws://127.0.0.1:{port}/ws"), socket).unwrap();
    assert_eq!(response.status(), 101);

    for i in 0..50 {
        ws.send(Message::text(format!("hello {i}"))).unwrap();
        assert_eq!(
            ws.read().unwrap(),
            Message::text(format!("echo: hello {i}"))
        );
    }
    let big = vec![7_u8; 256 * 1024];
    ws.send(Message::binary(big.clone())).unwrap();
    assert_eq!(ws.read().unwrap(), Message::binary(big));

    ws.close(None).unwrap();
    loop {
        match ws.read() {
            Ok(_) => {}
            Err(tungstenite::Error::ConnectionClosed) => break,
            Err(e) => panic!("close handshake: {e}"),
        }
    }
}

#[test]
fn a_plain_request_to_the_websocket_endpoint_gets_426() {
    let port = server();
    let socket = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let mut s = socket;
    use std::io::{Read, Write};
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.write_all(b"GET /ws HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .unwrap();
    let mut buf = [0_u8; 64];
    let n = s.read(&mut buf).unwrap();
    assert!(
        buf[..n].starts_with(b"HTTP/1.1 426"),
        "{}",
        String::from_utf8_lossy(&buf[..n])
    );
}
