//! Server-side protocol upgrade: `Response::upgrade` answers `101 Switching Protocols` and hands the
//! connection to the service once the 101 is written.
//!
//! The upgraded protocol here is a byte echo, so every test is a raw TCP client: the 101 head,
//! then bytes in and the same bytes out.

use may_minihttp::{HttpServer, HttpService, Request, Response};
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

#[derive(Clone)]
struct Svc;

impl HttpService for Svc {
    fn call(&mut self, req: Request, rsp: &mut Response) -> io::Result<()> {
        let asks_echo = req.headers().iter().any(|h| {
            h.name.eq_ignore_ascii_case("upgrade") && h.value.eq_ignore_ascii_case(b"echo")
        });
        match req.path() {
            "/echo" if asks_echo => {
                rsp.header("X-Echo: yes");
                rsp.body("a 101 has no body");
                rsp.upgrade("echo", |mut conn| {
                    conn.set_read_timeout(Some(Duration::from_secs(10)))?;
                    let mut buf = [0_u8; 1024];
                    loop {
                        match conn.read(&mut buf)? {
                            0 => return Ok(()),
                            n => conn.write_all(&buf[..n])?,
                        }
                    }
                });
                Ok(())
            }
            "/fails" => {
                rsp.upgrade("echo", |_| Ok(()));
                Err(io::Error::other(
                    "the service failed after asking to upgrade",
                ))
            }
            _ => {
                rsp.body("plain");
                Ok(())
            }
        }
    }
}

fn server() -> u16 {
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let _handle = HttpServer(Svc).start(("127.0.0.1", port)).unwrap();
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

fn connect(port: u16) -> TcpStream {
    let s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s
}

/// One response head, read byte by byte so nothing after it is consumed.
fn read_head(s: &mut TcpStream) -> String {
    let mut head = Vec::new();
    let mut byte = [0_u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        s.read_exact(&mut byte).expect("a response head");
        head.push(byte[0]);
    }
    String::from_utf8(head).unwrap()
}

fn read_n(s: &mut TcpStream, n: usize) -> Vec<u8> {
    let mut buf = vec![0_u8; n];
    s.read_exact(&mut buf).unwrap();
    buf
}

const UPGRADE: &str =
    "GET /echo HTTP/1.1\r\nHost: localhost\r\nUpgrade: echo\r\nConnection: Upgrade\r\n\r\n";

#[test]
fn upgrade_answers_101_and_hands_over_the_connection() {
    let port = server();
    let mut s = connect(port);
    // The request and the first bytes of the new protocol in one write: the server has them in its
    // request buffer when it switches, and the upgraded connection must still return them first.
    s.write_all(format!("{UPGRADE}early").as_bytes()).unwrap();

    let head = read_head(&mut s);
    let lower = head.to_ascii_lowercase();
    assert!(
        head.starts_with("HTTP/1.1 101 Switching Protocols\r\n"),
        "{head}"
    );
    assert!(lower.contains("\r\nupgrade: echo"), "{head}");
    assert!(lower.contains("\r\nconnection: upgrade"), "{head}");
    assert!(lower.contains("\r\nx-echo: yes"), "{head}");
    assert!(
        !lower.contains("content-length"),
        "a 101 has no Content-Length: {head}"
    );

    assert_eq!(read_n(&mut s, 5), b"early");
    s.write_all(b"ping").unwrap();
    assert_eq!(read_n(&mut s, 4), b"ping");
    s.write_all(b"GET / HTTP/1.1\r\n\r\n").unwrap();
    assert_eq!(
        read_n(&mut s, 18),
        b"GET / HTTP/1.1\r\n\r\n",
        "after the 101 the connection is the new protocol, never HTTP again"
    );
}

#[test]
fn pipelined_responses_before_the_upgrade_go_out_first() {
    let port = server();
    let mut s = connect(port);
    s.write_all(format!("GET /plain HTTP/1.1\r\nHost: localhost\r\n\r\n{UPGRADE}").as_bytes())
        .unwrap();

    let first = read_head(&mut s);
    assert!(first.starts_with("HTTP/1.1 200"), "{first}");
    assert_eq!(read_n(&mut s, 5), b"plain");
    let second = read_head(&mut s);
    assert!(second.starts_with("HTTP/1.1 101"), "{second}");
    s.write_all(b"after").unwrap();
    assert_eq!(read_n(&mut s, 5), b"after");
}

#[test]
fn an_upgrade_request_with_a_body_is_refused() {
    let port = server();
    let mut s = connect(port);
    s.write_all(
        b"POST /echo HTTP/1.1\r\nHost: localhost\r\nUpgrade: echo\r\nConnection: Upgrade\r\n\
          Content-Length: 4\r\n\r\nbody",
    )
    .unwrap();
    let head = read_head(&mut s);
    assert!(head.starts_with("HTTP/1.1 400"), "{head}");
    let mut rest = Vec::new();
    s.read_to_end(&mut rest).unwrap();
    assert!(rest.is_empty(), "the connection closes without switching");
}

#[test]
fn a_service_error_sends_its_500_and_does_not_switch() {
    let port = server();
    let mut s = connect(port);
    s.write_all(
        b"GET /fails HTTP/1.1\r\nHost: localhost\r\nUpgrade: echo\r\nConnection: Upgrade\r\n\r\n",
    )
    .unwrap();
    let head = read_head(&mut s);
    assert!(head.starts_with("HTTP/1.1 500"), "{head}");
    let mut rest = Vec::new();
    s.read_to_end(&mut rest).unwrap();
    assert!(
        !String::from_utf8_lossy(&rest).contains("101"),
        "no switch after a failed call"
    );
}

#[test]
fn a_request_that_does_not_ask_is_served_as_http() {
    let port = server();
    let mut s = connect(port);
    s.write_all(b"GET /echo HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .unwrap();
    let head = read_head(&mut s);
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(read_n(&mut s, 5), b"plain");
}
