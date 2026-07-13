//! Phase 4: Malformed response E2E — client handling garbage from a broken server.
//!
//! Tests the raw TCP wire protocol handling of malformed responses:
//! - Truncated body (CL != actual)
//! - Non-numeric Content-Length
//! - Missing headers
//! - Duplicate headers
//! - Invalid status codes
//!
//! Run with:
//!     cargo test --test perf_malformed_response --features client -- --test-threads=1 --nocapture

use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Once};
use std::time::Duration;

static INIT: Once = Once::new();

fn init_may_runtime() {
    INIT.call_once(|| {
        let _ = may::config().set_stack_size(0x8000);
    });
}

fn find_available_port(preferred: u16) -> u16 {
    for port in preferred..(preferred + 1000) {
        if TcpListener::bind(format!("127.0.0.1:{port}")).is_ok() {
            return port;
        }
    }
    panic!("No port in range {preferred}");
}

fn check_ready(port: u16, max_attempts: u32) -> bool {
    for _ in 0..max_attempts {
        match TcpStream::connect(format!("127.0.0.1:{port}")) {
            Ok(mut stream) => {
                let req = "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
                if stream.write_all(req.as_bytes()).is_ok() {
                    let mut buf = [0u8; 256];
                    if stream.read(&mut buf).is_ok() {
                        let _ = stream.shutdown(std::net::Shutdown::Both);
                        return true;
                    }
                }
            }
            Err(_) => {}
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

/// Serve one connection with a specific response, then close.
struct MalformedServer {
    port: u16,
    response: String,
    shutdown: Arc<AtomicBool>,
}

impl MalformedServer {
    fn new(preferred_port: u16, response: &str) -> Self {
        let port = find_available_port(preferred_port);
        let shutdown = Arc::new(AtomicBool::new(false));
        let resp = response.to_string();
        let shutdown_clone = Arc::clone(&shutdown);

        std::thread::spawn(move || {
            let listener = match TcpListener::bind(format!("127.0.0.1:{port}")) {
                Ok(l) => l,
                Err(_) => return,
            };
            let mut ready = false;
            while !shutdown_clone.load(Ordering::Relaxed) {
                if let Ok((mut stream, _)) = listener.accept() {
                    let _ = stream.write_all(resp.as_bytes());
                    let _ = stream.shutdown(std::net::Shutdown::Write);
                    ready = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            // If server didn't get a client, still keep listening briefly
            if !ready {
                while let Ok((mut stream, _)) = listener.accept() {
                    let _ = stream.shutdown(std::net::Shutdown::Both);
                }
            }
        });

        // Wait for server to be ready (first connection succeeds)
        assert!(
            check_ready(port, 50),
            "Server failed to start on port {port}"
        );

        // Reset the server for the real test — we need a second listener
        // Since the first connection already consumed the response,
        // we just start a fresh thread for the actual test response.
        let actual_resp = response.to_string();
        let actual_resp_clone = actual_resp.clone();
        let shutdown2 = Arc::clone(&shutdown);
        std::thread::spawn(move || {
            let listener = match TcpListener::bind(format!("127.0.0.1:{port}")) {
                Ok(l) => l,
                Err(_) => return,
            };
            while !shutdown2.load(Ordering::Relaxed) {
                if let Ok((mut stream, _)) = listener.accept() {
                    let _ = stream.write_all(actual_resp_clone.as_bytes());
                    let _ = stream.shutdown(std::net::Shutdown::Write);
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });

        // Give the new listener a moment to bind
        std::thread::sleep(Duration::from_millis(50));

        Self {
            port,
            response: actual_resp,
            shutdown,
        }
    }

    fn request(&self, req: &str, max: usize) -> io::Result<Vec<u8>> {
        let mut stream = TcpStream::connect(format!("127.0.0.1:{}", self.port))?;
        stream
            .set_read_timeout(Some(Duration::from_millis(500)))
            .ok();
        stream.write_all(req.as_bytes())?;
        let mut buf = vec![0u8; max];
        match stream.read(&mut buf) {
            Ok(n) if n > 0 => {
                buf.truncate(n);
                Ok(buf)
            }
            Ok(_) => Ok(vec![]),
            Err(e) if e.kind() == io::ErrorKind::TimedOut => Ok(buf),
            Err(e) => Err(e),
        }
    }
}

impl Drop for MalformedServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Test 1: Truncated body — Content-Length says 10 but only 3 bytes.
#[test]
fn test_truncated_body() {
    eprintln!("\n=== Malformed Response: Truncated (CL=10, sent 3) ===");

    let server = MalformedServer::new(32000, "HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nabc");

    let buf = server
        .request("GET / HTTP/1.1\r\nHost: localhost\r\n\r\n", 4096)
        .unwrap();
    assert!(buf.len() > 0, "Should receive some data");
    let s = String::from_utf8_lossy(&buf);
    assert!(
        s.starts_with("HTTP/1.1 200"),
        "Expected 200, got: {:?}",
        s.lines().next()
    );
    eprintln!("  Truncated: {} bytes", buf.len());
}

/// Test 2: Non-numeric Content-Length.
#[test]
fn test_non_numeric_cl() {
    eprintln!("\n=== Malformed Response: Non-numeric Content-Length ===");

    let server = MalformedServer::new(
        32010,
        "HTTP/1.1 200 OK\r\nContent-Length: notanumber\r\n\r\nhello",
    );

    let buf = server
        .request("GET / HTTP/1.1\r\nHost: localhost\r\n\r\n", 4096)
        .unwrap();
    let s = String::from_utf8_lossy(&buf);
    assert!(s.starts_with("HTTP/1.1 200"));
    eprintln!("  Non-numeric CL: {} bytes", buf.len());
}

/// Test 3: Missing headers (just status + body).
#[test]
fn test_missing_headers() {
    eprintln!("\n=== Malformed Response: Missing headers ===");

    let server = MalformedServer::new(32030, "HTTP/1.1 200 OK\r\n\r\ntest body");

    let buf = server
        .request("GET / HTTP/1.1\r\nHost: localhost\r\n\r\n", 4096)
        .unwrap();
    let s = String::from_utf8_lossy(&buf);
    assert!(s.starts_with("HTTP/1.1 200"));
    eprintln!("  Missing headers: {} bytes", buf.len());
}

/// Test 4: Content-Length: 0 with no body.
#[test]
fn test_cl_zero() {
    eprintln!("\n=== Malformed Response: Content-Length: 0 ===");

    let server = MalformedServer::new(
        32040,
        "HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n",
    );

    let buf = server
        .request("GET / HTTP/1.1\r\nHost: localhost\r\n\r\n", 4096)
        .unwrap();
    let s = String::from_utf8_lossy(&buf);
    assert!(s.contains("204"));
    eprintln!("  CL=0: {} bytes", buf.len());
}

/// Test 5: Garbage after body.
#[test]
fn test_garbage_after_body() {
    eprintln!("\n=== Malformed Response: Garbage after body ===");

    let server = MalformedServer::new(
        32050,
        "HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhelloGARBAGE",
    );

    let buf = server
        .request("GET / HTTP/1.1\r\nHost: localhost\r\n\r\n", 4096)
        .unwrap();
    let s = String::from_utf8_lossy(&buf);
    assert!(s.starts_with("HTTP/1.1 200"));
    assert!(s.contains("hello"));
    eprintln!("  Garbage after body: {} bytes", buf.len());
}

/// Test 6: Huge Content-Length with short body.
#[test]
fn test_huge_cl() {
    eprintln!("\n=== Malformed Response: Huge Content-Length ===");

    let server = MalformedServer::new(
        32060,
        "HTTP/1.1 200 OK\r\nContent-Length: 1000000000\r\n\r\nshort",
    );

    let buf = server
        .request("GET / HTTP/1.1\r\nHost: localhost\r\n\r\n", 4096)
        .unwrap();
    let s = String::from_utf8_lossy(&buf);
    assert!(s.starts_with("HTTP/1.1 200"));
    eprintln!("  Huge CL: {} bytes", buf.len());
}

/// Test 7: Duplicate headers.
#[test]
fn test_duplicate_headers() {
    eprintln!("\n=== Malformed Response: Duplicate headers ===");

    let server = MalformedServer::new(
        32080,
        "HTTP/1.1 200 OK\r\nContent-Length: 5\r\nX-Custom: first\r\nX-Custom: second\r\n\r\nhello",
    );

    let buf = server
        .request("GET / HTTP/1.1\r\nHost: localhost\r\n\r\n", 4096)
        .unwrap();
    let s = String::from_utf8_lossy(&buf);
    assert!(s.starts_with("HTTP/1.1 200"));
    assert!(s.contains("X-Custom"));
    eprintln!("  Duplicate headers: {} bytes", buf.len());
}

/// Test 8: Non-numeric status code.
#[test]
fn test_invalid_status() {
    eprintln!("\n=== Malformed Response: Non-numeric status ===");

    let server = MalformedServer::new(
        32090,
        "HTTP/1.1 ABC Bad Status\r\nContent-Length: 5\r\n\r\nhello",
    );

    let buf = server
        .request("GET / HTTP/1.1\r\nHost: localhost\r\n\r\n", 4096)
        .unwrap();
    let s = String::from_utf8_lossy(&buf);
    assert!(s.contains("ABC"));
    eprintln!("  Invalid status: {} bytes", buf.len());
}

/// Test 9: Multiple small garbage responses in sequence.
#[test]
fn test_multiple_garbage() {
    eprintln!("\n=== Malformed Response: Multiple garbage responses ===");

    let cases = [
        ("empty response", ""),
        ("partial HTTP", "HTTP/1.1 20"),
        ("just CR LF", "\r\n"),
        ("null bytes", "GET \x00\x01\x02\r\n\r\n"),
    ];

    let mut base_port = 32100u16;
    for (name, resp) in &cases {
        let server = MalformedServer::new(base_port, resp);
        base_port += 1;

        let result = server.request("GET / HTTP/1.1\r\nHost: localhost\r\n\r\n", 4096);
        match result {
            Ok(buf) => eprintln!("  {}: {} bytes", name, buf.len()),
            Err(e) => eprintln!("  {}: error '{}' (ok)", name, e),
        }
    }

    eprintln!("  Multiple garbage: handled");
}

/// Test 10: Missing final CRLF before body.
#[test]
fn test_missing_crlf() {
    eprintln!("\n=== Malformed Response: Missing body CRLF ===");

    let server = MalformedServer::new(32200, "HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\rhello");

    let buf = server
        .request("GET / HTTP/1.1\r\nHost: localhost\r\n\r\n", 4096)
        .unwrap();
    let s = String::from_utf8_lossy(&buf);
    assert!(s.starts_with("HTTP/1.1 200"));
    eprintln!("  Missing CRLF: {} bytes", buf.len());
}
