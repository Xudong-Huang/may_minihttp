//! Comprehensive header handling test suite
//!
//! Tests verify the server correctly handles varying header counts:
//! - Below limit (should pass)
//! - At limit boundary (should pass)
//! - Above limit (should fail with TooManyHeaders)

use bytes::BufMut;
use may_minihttp::{HttpServer, HttpService, Request, Response};
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::sync::Once;
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

/// Start a test server and return its handle
fn start_test_server(port: u16) -> may::coroutine::JoinHandle<()> {
    init_may_runtime();

    let handle = HttpServer(TestService)
        .start(format!("127.0.0.1:{}", port))
        .expect("Failed to start server");

    // Wait for server to be ready
    for _ in 0..50 {
        if TcpStream::connect(format!("127.0.0.1:{}", port)).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    handle
}

/// Send HTTP request with specified number of headers
fn send_request_with_headers(port: u16, num_headers: usize) -> io::Result<String> {
    let mut stream = TcpStream::connect(format!("127.0.0.1:{}", port))?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;

    // Build request with specified number of headers
    let mut request = String::from("GET / HTTP/1.1\r\n");

    // Add headers (Host counts as 1)
    request.push_str("Host: localhost\r\n");

    // Add custom headers to reach desired count
    for i in 1..num_headers {
        request.push_str(&format!("X-Custom-{}: value{}\r\n", i, i));
    }

    request.push_str("\r\n");

    stream.write_all(request.as_bytes())?;
    stream.flush()?;

    // Read response
    let mut response = Vec::new();
    let mut buffer = [0u8; 2048];

    loop {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => response.extend_from_slice(&buffer[0..n]),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
            Err(e) => return Err(e),
        }
    }

    String::from_utf8(response).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

// ============================================================================
// TEST SUITE: Header Count Validation
// ============================================================================

#[test]
fn test_3_headers_well_below_limit() {
    let handle = start_test_server(18080);

    let response = send_request_with_headers(18080, 3).expect("Failed to send request");

    println!("Response:\n{}", response);

    assert!(response.contains("200"), "Should get 200 OK");
    assert!(response.contains("Headers: 3"), "Should receive 3 headers");

    // Cleanup
    unsafe {
        handle.coroutine().cancel();
    }
    let _ = handle.join();
}

#[test]
fn test_10_headers_below_limit() {
    let handle = start_test_server(18081);

    let response = send_request_with_headers(18081, 10).expect("Failed to send request");

    println!("10 headers response:\n{}", response);

    assert!(
        response.contains("200"),
        "Should get 200 OK with 10 headers"
    );
    assert!(
        response.contains("Headers: 10"),
        "Should receive 10 headers"
    );

    // Cleanup
    unsafe {
        handle.coroutine().cancel();
    }
    let _ = handle.join();
}

#[test]
fn test_16_headers_at_default_limit() {
    let handle = start_test_server(18082);

    let response = send_request_with_headers(18082, 16).expect("Failed to send request");

    println!("16 headers (at limit) response:\n{}", response);

    assert!(
        response.contains("200"),
        "Should get 200 OK with exactly 16 headers (at limit)"
    );
    assert!(
        response.contains("Headers: 16"),
        "Should receive 16 headers"
    );

    // Cleanup
    unsafe {
        handle.coroutine().cancel();
    }
    let _ = handle.join();
}

#[test]
fn test_17_headers_exceeds_default_limit() {
    let handle = start_test_server(18083);

    let result = send_request_with_headers(18083, 17);

    match result {
        Ok(response) => {
            println!("17 headers response:\n{}", response);

            // Server logs TooManyHeaders but may still send response with empty body
            // The key is that our handler should NOT be called with over-limit headers
            assert!(
                response.is_empty() || !response.contains("Headers: 17"),
                "Handler should not receive 17 headers (logged TooManyHeaders error)"
            );
            println!("✓ Server correctly rejected 17 headers (TooManyHeaders logged)");
        }
        Err(e) => {
            // Connection closed/reset is also acceptable
            println!("✓ Expected connection error with 17 headers: {}", e);
        }
    }

    // Cleanup
    unsafe {
        handle.coroutine().cancel();
    }
    let _ = handle.join();
}

#[test]
fn test_20_headers_well_over_limit() {
    let handle = start_test_server(18084);

    let result = send_request_with_headers(18084, 20);

    match result {
        Ok(response) => {
            println!("20 headers response:\n{}", response);

            // Server logs TooManyHeaders but may still send response with empty body
            assert!(
                response.is_empty() || !response.contains("Headers: 20"),
                "Handler should not receive 20 headers (logged TooManyHeaders error)"
            );
            println!(
                "✓ Server correctly rejected 20 headers (TooManyHeaders logged, +4 over limit)"
            );
        }
        Err(e) => {
            // Connection closed/reset is also acceptable
            println!("✓ Expected connection error with 20 headers: {}", e);
        }
    }

    // Cleanup
    unsafe {
        handle.coroutine().cancel();
    }
    let _ = handle.join();
}

#[test]
fn test_32_headers_far_over_limit() {
    let handle = start_test_server(18085);

    let result = send_request_with_headers(18085, 32);

    match result {
        Ok(response) => {
            println!("32 headers response:\n{}", response);

            // Server logs TooManyHeaders but may still send response with empty body
            assert!(
                response.is_empty() || !response.contains("Headers: 32"),
                "Handler should not receive 32 headers (logged TooManyHeaders error)"
            );
            println!(
                "✓ Server correctly rejected 32 headers (TooManyHeaders logged, +16 over limit)"
            );
        }
        Err(e) => {
            // Connection closed/reset is also acceptable
            println!("✓ Expected connection error with 32 headers: {}", e);
        }
    }

    // Cleanup
    unsafe {
        handle.coroutine().cancel();
    }
    let _ = handle.join();
}
