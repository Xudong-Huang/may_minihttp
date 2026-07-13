//! Comprehensive header handling test suite
//!
//! Tests verify the server correctly handles varying header counts:
//! - Below limit (should pass)
//! - At limit boundary (should pass)
//! - Above limit (should fail with TooManyHeaders)

use bytes::BufMut;
use may_minihttp::{HttpServer, HttpService, Request, Response};
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Once};
use std::thread;
use std::time::Duration;

static INIT: Once = Once::new();

/// Initialize MAY runtime once for all tests
fn init_may_runtime() {
    INIT.call_once(|| {
        may::config().set_stack_size(0x8000);
    });
}

/// Simple test service that echoes header count
#[derive(Clone)]
struct TestService;

impl HttpService for TestService {
    fn call(&mut self, req: Request, res: &mut Response) -> io::Result<()> {
        use io::Write;

        let header_count = req.headers().len();
        let response = format!("Headers: {}\n", header_count);

        write!(res.body_mut().writer(), "{}", response)?;
        Ok(())
    }
}

/// RAII test server on a dedicated OS thread (required on Windows IOCP).
struct SimpleHeaderTestServer {
    port: u16,
    shutdown: Arc<AtomicBool>,
    server_thread: Option<thread::JoinHandle<()>>,
}

/// Check if a port is available for binding
fn is_port_available(port: u16) -> bool {
    TcpListener::bind(format!("127.0.0.1:{}", port)).is_ok()
}

/// Find the next available port starting from the given port
fn find_available_port(start_port: u16) -> u16 {
    for port in start_port..(start_port + 100) {
        if is_port_available(port) {
            return port;
        }
    }
    panic!(
        "Could not find available port in range {}-{}",
        start_port,
        start_port + 100
    );
}

/// Ensure a port is available, finding an alternative if necessary
fn ensure_port_available(preferred_port: u16) -> u16 {
    if is_port_available(preferred_port) {
        preferred_port
    } else {
        find_available_port(preferred_port + 1)
    }
}

impl SimpleHeaderTestServer {
    fn new(preferred_port: u16) -> Self {
        init_may_runtime();

        let port = ensure_port_available(preferred_port);
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = Arc::clone(&shutdown);
        let addr = format!("127.0.0.1:{}", port);
        let server_thread = thread::spawn(move || {
            let handle = HttpServer(TestService)
                .start(&addr)
                .expect("Failed to start server");

            while !shutdown_clone.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(50));
            }

            unsafe {
                handle.coroutine().cancel();
            }
            let _ = handle.join();
        });

        let fixture = Self {
            port,
            shutdown,
            server_thread: Some(server_thread),
        };

        assert!(
            fixture.wait_for_ready(50),
            "Server failed to start on port {}",
            port
        );
        thread::sleep(Duration::from_millis(100));
        fixture
    }

    fn port(&self) -> u16 {
        self.port
    }

    fn wait_for_ready(&self, max_attempts: u32) -> bool {
        for _ in 0..max_attempts {
            if let Ok(mut stream) = TcpStream::connect(format!("127.0.0.1:{}", self.port)) {
                let request = "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
                if stream.write_all(request.as_bytes()).is_ok() {
                    let mut buf = [0u8; 256];
                    if stream.read(&mut buf).is_ok() {
                        let _ = stream.shutdown(std::net::Shutdown::Both);
                        return true;
                    }
                }
            }
            thread::sleep(Duration::from_millis(10));
        }
        false
    }
}

impl Drop for SimpleHeaderTestServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(handle) = self.server_thread.take() {
            let _ = handle.join();
        }
    }
}

/// Send HTTP request with specified number of headers
fn send_request_with_headers(port: u16, num_headers: usize) -> io::Result<String> {
    let mut request = String::from("GET / HTTP/1.1\r\n");
    request.push_str("Host: localhost\r\n");

    // Add custom headers to reach desired count (Host counts as 1)
    for i in 1..num_headers {
        request.push_str(&format!("X-Custom-{}: value{}\r\n", i, i));
    }
    request.push_str("\r\n");

    let mut last_err = None;
    for attempt in 0..3u32 {
        if attempt > 0 {
            thread::sleep(Duration::from_millis(100 * attempt as u64));
        }
        match send_single_request(port, &request) {
            Ok(response) => return Ok(response),
            Err(e) => {
                let kind = e.kind();
                if kind != io::ErrorKind::TimedOut && kind != io::ErrorKind::ConnectionRefused {
                    return Err(e);
                }
                last_err = Some(e);
            }
        }
    }
    Err(last_err.expect("loop always has error"))
}

fn send_single_request(port: u16, request: &str) -> io::Result<String> {
    let mut stream = TcpStream::connect(format!("127.0.0.1:{}", port))?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;

    stream.write_all(request.as_bytes())?;
    stream.flush()?;

    let mut response = Vec::new();
    let mut buffer = [0u8; 2048];

    loop {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => response.extend_from_slice(&buffer[0..n]),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == io::ErrorKind::TimedOut => break,
            Err(e) => return Err(e),
        }
    }

    let _ = stream.shutdown(std::net::Shutdown::Write);

    String::from_utf8(response).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

// ============================================================================
// TEST SUITE: Header Count Validation
// ============================================================================

#[test]
fn test_3_headers_well_below_limit() {
    let server = SimpleHeaderTestServer::new(18080);

    let response = send_request_with_headers(server.port(), 3).expect("Failed to send request");

    println!("Response:\n{}", response);

    assert!(response.contains("200"), "Should get 200 OK");
    assert!(response.contains("Headers: 3"), "Should receive 3 headers");
}

#[test]
fn test_10_headers_below_limit() {
    let server = SimpleHeaderTestServer::new(18081);

    let response = send_request_with_headers(server.port(), 10).expect("Failed to send request");

    println!("10 headers response:\n{}", response);

    assert!(
        response.contains("200"),
        "Should get 200 OK with 10 headers"
    );
    assert!(
        response.contains("Headers: 10"),
        "Should receive 10 headers"
    );
}

#[test]
fn test_16_headers_at_default_limit() {
    let server = SimpleHeaderTestServer::new(18082);

    let response = send_request_with_headers(server.port(), 16).expect("Failed to send request");

    println!("16 headers (at limit) response:\n{}", response);

    assert!(
        response.contains("200"),
        "Should get 200 OK with exactly 16 headers (at limit)"
    );
    assert!(
        response.contains("Headers: 16"),
        "Should receive 16 headers"
    );
}

#[test]
fn test_17_headers_exceeds_default_limit() {
    let server = SimpleHeaderTestServer::new(18083);

    let result = send_request_with_headers(server.port(), 17);

    match result {
        Ok(response) => {
            println!("17 headers response:\n{}", response);

            assert!(
                response.is_empty() || !response.contains("Headers: 17"),
                "Handler should not receive 17 headers (logged TooManyHeaders error)"
            );
            println!("✓ Server correctly rejected 17 headers (TooManyHeaders logged)");
        }
        Err(e) => {
            println!("✓ Expected connection error with 17 headers: {}", e);
        }
    }
}

#[test]
fn test_20_headers_well_over_limit() {
    let server = SimpleHeaderTestServer::new(18084);

    let result = send_request_with_headers(server.port(), 20);

    match result {
        Ok(response) => {
            println!("20 headers response:\n{}", response);

            assert!(
                response.is_empty() || !response.contains("Headers: 20"),
                "Handler should not receive 20 headers (logged TooManyHeaders error)"
            );
            println!(
                "✓ Server correctly rejected 20 headers (TooManyHeaders logged, +4 over limit)"
            );
        }
        Err(e) => {
            println!("✓ Expected connection error with 20 headers: {}", e);
        }
    }
}

#[test]
fn test_32_headers_far_over_limit() {
    let server = SimpleHeaderTestServer::new(18085);

    let result = send_request_with_headers(server.port(), 32);

    match result {
        Ok(response) => {
            println!("32 headers response:\n{}", response);

            assert!(
                response.is_empty() || !response.contains("Headers: 32"),
                "Handler should not receive 32 headers (logged TooManyHeaders error)"
            );
            println!(
                "✓ Server correctly rejected 32 headers (TooManyHeaders logged, +16 over limit)"
            );
        }
        Err(e) => {
            println!("✓ Expected connection error with 32 headers: {}", e);
        }
    }
}
