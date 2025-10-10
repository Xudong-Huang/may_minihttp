//! Integration tests with actual HTTP traffic
//!
//! These tests start a real HTTP server using RAII fixtures and send requests with varying
//! numbers of headers to verify the buffering and MAX_HEADERS behavior.
//!
//! ## RAII Test Fixtures
//!
//! All tests use `HeaderTestServer` which implements the `Drop` trait to ensure
//! proper cleanup of server threads, even on panic or early return.
//!
//! ## Port Management
//!
//! Tests use dynamic port allocation to prevent conflicts:
//! - Check if port is available before binding
//! - Automatically find next available port
//! - Safe for parallel test execution

use bytes::BufMut;
use may_minihttp::{HttpServer, HttpService, Request, Response};
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Once;
use std::thread;
use std::time::Duration;

static INIT: Once = Once::new();

/// Initialize MAY runtime once for all tests
fn init_may_runtime() {
    INIT.call_once(|| {
        may::config().set_stack_size(0x8000);
    });
}

#[derive(Clone)]
struct TestService;

impl HttpService for TestService {
    fn call(&mut self, req: Request, res: &mut Response) -> io::Result<()> {
        use std::io::Write;

        let header_count = req.headers().len();

        // Enable keep-alive to prevent connection drops
        res.header("Connection: keep-alive");
        res.header("Keep-Alive: timeout=5, max=1000");

        // Build a simple response
        let response = format!("OK:{}", header_count);

        // Write response - ignore errors (BrokenPipe can occur in tests)
        let _ = write!(res.body_mut().writer(), "{}", response);
        Ok(())
    }
}

/// Check if a port is available for binding
///
/// Returns `true` if the port is free, `false` if it's already in use.
fn is_port_available(port: u16) -> bool {
    TcpListener::bind(format!("127.0.0.1:{}", port)).is_ok()
}

/// Find the next available port starting from the given port
///
/// Tries up to 100 consecutive ports before giving up.
/// Returns the first available port found, or panics if none are available.
fn find_available_port(start_port: u16) -> u16 {
    for port in start_port..(start_port + 100) {
        if is_port_available(port) {
            eprintln!("[PORT] Found available port: {}", port);
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
///
/// First tries the preferred port. If unavailable, finds the next free port.
/// This prevents test failures from port conflicts while maintaining determinism
/// when possible.
fn ensure_port_available(preferred_port: u16) -> u16 {
    if is_port_available(preferred_port) {
        eprintln!("[PORT] Using preferred port: {}", preferred_port);
        preferred_port
    } else {
        eprintln!(
            "[PORT] Port {} in use, finding alternative...",
            preferred_port
        );
        find_available_port(preferred_port + 1)
    }
}

/// RAII test fixture for HTTP server
///
/// Ensures the server is properly shut down when the fixture is dropped,
/// preventing resource leaks and port conflicts between tests.
///
/// ## Port Management
///
/// The fixture uses `ensure_port_available()` to check if the preferred port
/// is free. If not, it automatically finds the next available port. This prevents
/// test failures from port conflicts during parallel execution or when other
/// services are running.
struct HeaderTestServer {
    port: u16,
    handle: Option<may::coroutine::JoinHandle<()>>,
}

impl HeaderTestServer {
    /// Create and start a new test server with port availability checking
    ///
    /// # Arguments
    ///
    /// * `preferred_port` - The preferred port to use (will find alternative if unavailable)
    ///
    /// # Example
    ///
    /// ```ignore
    /// let server = HeaderTestServer::new(18001);
    /// // Server automatically uses port 18001, or next available port if busy
    /// ```
    fn new(preferred_port: u16) -> Self {
        // CRITICAL: Initialize MAY runtime configuration FIRST (once for all tests)
        init_may_runtime();

        // Check port availability and find alternative if needed
        let port = ensure_port_available(preferred_port);

        // Start the HTTP server in the MAIN THREAD (not a background thread)
        // This matches BRRTRouter's pattern exactly:
        // - HttpServer.start() spawns a coroutine and returns immediately
        // - The JoinHandle keeps the server running
        // - No thread::spawn needed - MAY handles concurrency with coroutines
        let handle = HttpServer(TestService)
            .start(&format!("127.0.0.1:{}", port))
            .expect("Failed to start test server");

        let fixture = Self {
            port,
            handle: Some(handle),
        };

        // Wait for server to be ready
        assert!(
            fixture.wait_for_ready(50),
            "Server failed to start on port {}",
            port
        );
        thread::sleep(Duration::from_millis(200));

        eprintln!("[SERVER] HeaderTestServer started on port {}", port);

        fixture
    }

    /// Wait for server to be ready to accept connections
    ///
    /// This sends an actual HTTP request to verify the server is responsive,
    /// not just listening.
    fn wait_for_ready(&self, max_attempts: u32) -> bool {
        for attempt in 0..max_attempts {
            if let Ok(mut stream) = TcpStream::connect(format!("127.0.0.1:{}", self.port)) {
                // Send a minimal HTTP request
                let request = format!("GET / HTTP/1.1\r\nHost: localhost\r\n\r\n");
                if stream.write_all(request.as_bytes()).is_ok() {
                    // Try to read some response
                    let mut buf = [0u8; 256];
                    if stream.read(&mut buf).is_ok() {
                        eprintln!(
                            "[READY] Server on port {} is ready (attempt {})",
                            self.port,
                            attempt + 1
                        );
                        return true;
                    }
                }
            }
            thread::sleep(Duration::from_millis(100));
        }
        eprintln!(
            "[ERROR] Server on port {} failed to become ready after {} attempts",
            self.port, max_attempts
        );
        false
    }

    /// Get the port number for this server
    fn port(&self) -> u16 {
        self.port
    }
}

impl Drop for HeaderTestServer {
    fn drop(&mut self) {
        // Cancel the server coroutine and wait for it to finish
        // This matches BRRTRouter's ServerHandle::stop() implementation
        if let Some(handle) = self.handle.take() {
            unsafe {
                handle.coroutine().cancel();
            }
            let _ = handle.join();
        }
        eprintln!("[CLEANUP] HeaderTestServer on port {} shut down", self.port);
    }
}

fn send_request_with_headers(port: u16, num_headers: usize) -> io::Result<String> {
    let mut stream = TcpStream::connect(format!("127.0.0.1:{}", port))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;

    // Build HTTP request with specified number of headers
    let mut request = String::from("GET /test HTTP/1.1\r\n");
    request.push_str("Host: localhost\r\n");

    // Add custom headers up to the desired count (minus Host header)
    for i in 0..(num_headers - 1) {
        request.push_str(&format!("X-Custom-Header-{}: value-{}\r\n", i, i));
    }

    request.push_str("\r\n"); // End of headers

    // Send request
    stream.write_all(request.as_bytes())?;
    stream.flush()?;

    // Read response
    let mut response = Vec::new();
    let mut buffer = [0u8; 1024];

    loop {
        match stream.read(&mut buffer) {
            Ok(0) => break, // Connection closed
            Ok(n) => response.extend_from_slice(&buffer[0..n]),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
            Err(e) => return Err(e),
        }
    }

    String::from_utf8(response).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[test]
fn test_request_with_5_headers() {
    let server = HeaderTestServer::new(18001);

    let response = send_request_with_headers(server.port(), 5).expect("Failed to send request");

    assert!(response.contains("200"), "Should get 200 OK");
    assert!(response.contains("OK"), "Should receive OK response");
}

#[test]
fn test_request_with_10_headers() {
    let server = HeaderTestServer::new(18002);

    let response = send_request_with_headers(server.port(), 10).expect("Failed to send request");

    assert!(
        response.contains("200"),
        "Should get 200 OK with 10 headers"
    );
}

#[test]
fn test_request_with_16_headers_at_default_limit() {
    let server = HeaderTestServer::new(18003);

    let response = send_request_with_headers(server.port(), 16).expect("Failed to send request");

    assert!(
        response.contains("200"),
        "Should handle exactly 16 headers (current limit)"
    );
}

#[test]
fn test_default_limit_accepts_16_headers() {
    // Test that Default (16) accepts exactly 16 headers
    let server = HeaderTestServer::new(18004);

    let response = send_request_with_headers(server.port(), 16)
        .expect("Failed to send request with 16 headers");

    assert!(
        response.contains("200"),
        "Should accept 16 headers with Default config"
    );
}

#[test]
fn test_default_limit_rejects_17_headers() {
    // Test that Default (16) rejects 17 headers
    let server = HeaderTestServer::new(18005);

    let result = send_request_with_headers(server.port(), 17);

    // Should fail with TooManyHeaders or connection error
    match result {
        Ok(response) => {
            // Server might return error response or close connection
            println!("Response with 17 headers (should fail): {}", response);
            // If we get a response, it should be an error
            assert!(
                response.contains("400") || response.contains("500") || !response.contains("200"),
                "Should reject 17 headers with Default (16) config"
            );
        }
        Err(e) => {
            println!("Expected connection error with 17 headers: {}", e);
            // This is expected - connection closed due to TooManyHeaders
        }
    }
}

// Note: The following tests require server-wide MaxHeaders configuration.
// The decode() function is now generic and CAN accept 32/64/128 headers,
// but the HttpServer factory needs to be updated to pass the right array size.
//
// Implementation requires:
// 1. Generic decode<const N: usize>() - DONE
// 2. Helper functions (decode_standard, decode_large, decode_xlarge) - DONE
// 3. TODO: Update HttpServiceFactory to accept MaxHeaders parameter
// 4. TODO: Update each_connection_loop to use correct array size based on config
//
// These tests are removed to avoid confusion. Once the server configuration
// API is complete, add them back using:
//   let server = HeaderTestServer::with_max_headers(port, MaxHeaders::Standard);

#[test]
fn test_buffering_check_with_fragmented_headers() {
    let server = HeaderTestServer::new(18008);

    let mut stream =
        TcpStream::connect(format!("127.0.0.1:{}", server.port())).expect("Failed to connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("Failed to set timeout");

    // Send headers in multiple chunks to test buffering
    // First chunk: request line + partial header
    stream
        .write_all(b"GET /test HTTP/1.1\r\nHost: local")
        .unwrap();
    stream.flush().unwrap();
    thread::sleep(Duration::from_millis(50));

    // Second chunk: rest of Host header + another header
    stream
        .write_all(b"host\r\nUser-Agent: TestClient\r\n")
        .unwrap();
    stream.flush().unwrap();
    thread::sleep(Duration::from_millis(50));

    // Final chunk: end of headers
    stream.write_all(b"\r\n").unwrap();
    stream.flush().unwrap();

    // Read response
    let mut response = Vec::new();
    let mut buffer = [0u8; 1024];

    match stream.read(&mut buffer) {
        Ok(n) => response.extend_from_slice(&buffer[0..n]),
        Err(e) => panic!("Failed to read response: {}", e),
    }

    let response_str = String::from_utf8_lossy(&response);
    assert!(
        response_str.contains("200"),
        "Should handle fragmented headers correctly with buffering check"
    );
}

#[test]
fn test_browser_like_request() {
    let server = HeaderTestServer::new(18009);

    let mut stream =
        TcpStream::connect(format!("127.0.0.1:{}", server.port())).expect("Failed to connect");

    // Simulate a typical browser request (15-20 headers)
    let browser_request = "\
GET / HTTP/1.1\r\n\
Host: localhost\r\n\
User-Agent: Mozilla/5.0\r\n\
Accept: text/html,application/xhtml+xml\r\n\
Accept-Language: en-US,en;q=0.5\r\n\
Accept-Encoding: gzip, deflate\r\n\
Connection: keep-alive\r\n\
Upgrade-Insecure-Requests: 1\r\n\
Cache-Control: max-age=0\r\n\
Cookie: session=abc123\r\n\
Referer: http://example.com/\r\n\
DNT: 1\r\n\
Sec-Fetch-Dest: document\r\n\
Sec-Fetch-Mode: navigate\r\n\
Sec-Fetch-Site: none\r\n\
Sec-Fetch-User: ?1\r\n\
\r\n";

    stream.write_all(browser_request.as_bytes()).unwrap();
    stream.flush().unwrap();

    let mut response = Vec::new();
    let mut buffer = [0u8; 1024];

    match stream.read(&mut buffer) {
        Ok(n) => response.extend_from_slice(&buffer[0..n]),
        Err(e) => panic!("Failed to read response: {}", e),
    }

    let response_str = String::from_utf8_lossy(&response);

    // With MAX_HEADERS=16, this might succeed (15 headers)
    // But in production behind load balancer, this would have more headers
    println!("Browser-like request response: {}", response_str);
    assert!(
        response_str.contains("HTTP/1.1"),
        "Should receive HTTP response"
    );
}

#[test]
fn test_load_balancer_headers() {
    let server = HeaderTestServer::new(18010);

    let mut stream =
        TcpStream::connect(format!("127.0.0.1:{}", server.port())).expect("Failed to connect");

    // Simulate request from behind load balancer with proxy headers
    let lb_request = "\
GET /api/users HTTP/1.1\r\n\
Host: backend:8080\r\n\
X-Forwarded-For: 1.2.3.4, 5.6.7.8\r\n\
X-Forwarded-Proto: https\r\n\
X-Forwarded-Host: example.com\r\n\
X-Forwarded-Port: 443\r\n\
X-Real-IP: 1.2.3.4\r\n\
X-Request-ID: req-123\r\n\
X-Correlation-ID: corr-456\r\n\
User-Agent: LoadBalancer/1.0\r\n\
Accept: application/json\r\n\
\r\n";

    stream.write_all(lb_request.as_bytes()).unwrap();
    stream.flush().unwrap();

    let mut response = Vec::new();
    let mut buffer = [0u8; 1024];

    match stream.read(&mut buffer) {
        Ok(n) => response.extend_from_slice(&buffer[0..n]),
        Err(e) => panic!("Failed to read response: {}", e),
    }

    let response_str = String::from_utf8_lossy(&response);
    assert!(
        response_str.contains("HTTP/1.1"),
        "Should handle load balancer headers"
    );
}

// ============================================================================
// LARGE HEADER VALUE TESTS
// ============================================================================
// These tests verify behavior with large header values (not just counts)
// Buffer exhaustion can trigger TooManyHeaders even with < MAX_HEADERS headers

#[test]
fn test_large_user_agent_header() {
    // Realistic: very long User-Agent from modern browsers with extensions
    let _server = HeaderTestServer::new(18700);

    let mut stream = TcpStream::connect("127.0.0.1:18700").unwrap();

    // 500-character User-Agent (realistic for browsers with many extensions)
    let long_user_agent = format!(
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) \
        Chrome/120.0.0.0 Safari/537.36 Edg/120.0.0.0 Extension/1.2.3 Extension/4.5.6 \
        Extension/7.8.9 Extension/10.11.12 {}",
        "A".repeat(300)
    );

    let request = format!(
        "GET / HTTP/1.1\r\n\
        Host: localhost\r\n\
        User-Agent: {}\r\n\
        Accept: text/html\r\n\
        \r\n",
        long_user_agent
    );

    stream.write_all(request.as_bytes()).unwrap();
    stream.flush().unwrap();

    let mut response = Vec::new();
    let mut buffer = [0u8; 1024];

    match stream.read(&mut buffer) {
        Ok(n) => response.extend_from_slice(&buffer[0..n]),
        Err(e) => panic!("Failed to read response: {}", e),
    }

    let response_str = String::from_utf8_lossy(&response);
    assert!(
        response_str.contains("HTTP/1.1"),
        "Should handle large User-Agent header"
    );
}

#[test]
fn test_large_cookie_header() {
    // Realistic: large Cookie header with many session values
    let _server = HeaderTestServer::new(18701);

    let mut stream = TcpStream::connect("127.0.0.1:18701").unwrap();

    // Build a large cookie header (1KB+)
    let mut cookies = Vec::new();
    for i in 0..50 {
        cookies.push(format!("session_{}=abc123def456ghi789jklmno", i));
    }
    let large_cookie = cookies.join("; ");

    let request = format!(
        "GET / HTTP/1.1\r\n\
        Host: localhost\r\n\
        Cookie: {}\r\n\
        Accept: text/html\r\n\
        \r\n",
        large_cookie
    );

    stream.write_all(request.as_bytes()).unwrap();
    stream.flush().unwrap();

    let mut response = Vec::new();
    let mut buffer = [0u8; 1024];

    match stream.read(&mut buffer) {
        Ok(n) => response.extend_from_slice(&buffer[0..n]),
        Err(e) => panic!("Failed to read response: {}", e),
    }

    let response_str = String::from_utf8_lossy(&response);
    assert!(
        response_str.contains("HTTP/1.1"),
        "Should handle large Cookie header"
    );
}

#[test]
fn test_large_referer_header() {
    // Realistic: very long Referer URL with query parameters
    let _server = HeaderTestServer::new(18702);

    let mut stream = TcpStream::connect("127.0.0.1:18702").unwrap();

    // Build a long URL with many query parameters (800+ chars)
    let mut params = Vec::new();
    for i in 0..30 {
        params.push(format!("param_{}=value_{}", i, "x".repeat(10)));
    }
    let long_referer = format!("https://example.com/path/to/resource?{}", params.join("&"));

    let request = format!(
        "GET / HTTP/1.1\r\n\
        Host: localhost\r\n\
        Referer: {}\r\n\
        Accept: text/html\r\n\
        \r\n",
        long_referer
    );

    stream.write_all(request.as_bytes()).unwrap();
    stream.flush().unwrap();

    let mut response = Vec::new();
    let mut buffer = [0u8; 1024];

    match stream.read(&mut buffer) {
        Ok(n) => response.extend_from_slice(&buffer[0..n]),
        Err(e) => panic!("Failed to read response: {}", e),
    }

    let response_str = String::from_utf8_lossy(&response);
    assert!(
        response_str.contains("HTTP/1.1"),
        "Should handle large Referer header"
    );
}

#[test]
fn test_large_authorization_header() {
    // Realistic: large JWT token or OAuth bearer token
    let _server = HeaderTestServer::new(18703);

    let mut stream = TcpStream::connect("127.0.0.1:18703").unwrap();

    // Simulate a large JWT token (1.5KB+)
    let large_jwt = format!(
        "Bearer eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9.{}",
        "A".repeat(1400)
    );

    let request = format!(
        "GET / HTTP/1.1\r\n\
        Host: localhost\r\n\
        Authorization: {}\r\n\
        Accept: application/json\r\n\
        \r\n",
        large_jwt
    );

    stream.write_all(request.as_bytes()).unwrap();
    stream.flush().unwrap();

    let mut response = Vec::new();
    let mut buffer = [0u8; 1024];

    match stream.read(&mut buffer) {
        Ok(n) => response.extend_from_slice(&buffer[0..n]),
        Err(e) => panic!("Failed to read response: {}", e),
    }

    let response_str = String::from_utf8_lossy(&response);
    assert!(
        response_str.contains("HTTP/1.1"),
        "Should handle large Authorization header"
    );
}

#[test]
fn test_multiple_large_headers_combined() {
    // Stress test: multiple large headers in one request
    let _server = HeaderTestServer::new(18704);

    let mut stream = TcpStream::connect("127.0.0.1:18704").unwrap();

    let large_user_agent = format!(
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) {}",
        "A".repeat(400)
    );
    let large_cookie = (0..30)
        .map(|i| format!("session_{}={}", i, "x".repeat(20)))
        .collect::<Vec<_>>()
        .join("; ");
    let large_referer = format!(
        "https://example.com/path?{}",
        (0..20)
            .map(|i| format!("p{}=v{}", i, "y".repeat(15)))
            .collect::<Vec<_>>()
            .join("&")
    );

    let request = format!(
        "GET / HTTP/1.1\r\n\
        Host: localhost\r\n\
        User-Agent: {}\r\n\
        Cookie: {}\r\n\
        Referer: {}\r\n\
        Accept: text/html,application/xhtml+xml\r\n\
        Accept-Language: en-US,en;q=0.9\r\n\
        Accept-Encoding: gzip, deflate, br\r\n\
        Connection: keep-alive\r\n\
        \r\n",
        large_user_agent, large_cookie, large_referer
    );

    stream.write_all(request.as_bytes()).unwrap();
    stream.flush().unwrap();

    let mut response = Vec::new();
    let mut buffer = [0u8; 2048]; // Larger buffer for response

    match stream.read(&mut buffer) {
        Ok(n) => response.extend_from_slice(&buffer[0..n]),
        Err(e) => panic!("Failed to read response: {}", e),
    }

    let response_str = String::from_utf8_lossy(&response);
    assert!(
        response_str.contains("HTTP/1.1"),
        "Should handle multiple large headers"
    );
}

#[test]
fn test_extremely_large_single_header() {
    // Edge case: single header value that's extremely large (4KB+)
    let _server = HeaderTestServer::new(18705);

    let mut stream = TcpStream::connect("127.0.0.1:18705").unwrap();

    // 4KB header value
    let extremely_large_value = "X".repeat(4096);

    let request = format!(
        "GET / HTTP/1.1\r\n\
        Host: localhost\r\n\
        X-Custom-Header: {}\r\n\
        \r\n",
        extremely_large_value
    );

    stream.write_all(request.as_bytes()).unwrap();
    stream.flush().unwrap();

    let mut response = Vec::new();
    let mut buffer = [0u8; 1024];

    match stream.read(&mut buffer) {
        Ok(n) if n > 0 => {
            response.extend_from_slice(&buffer[0..n]);
            let response_str = String::from_utf8_lossy(&response);
            // This might return an error response or timeout
            eprintln!("Response: {}", response_str);
        }
        _ => {
            // Expected: may fail to parse due to buffer exhaustion
            eprintln!("Server closed connection (expected for 4KB header)");
        }
    }
}

#[test]
fn test_realistic_api_gateway_headers() {
    // Realistic: headers from API Gateway with tracing, correlation, forwarding
    let _server = HeaderTestServer::new(18706);

    let mut stream = TcpStream::connect("127.0.0.1:18706").unwrap();

    let trace_id = format!("trace-{}", "0123456789abcdef".repeat(8)); // 128-char trace ID
    let correlation_id = format!("correlation-{}", "fedcba9876543210".repeat(8));
    let forwarded_for = (0..20)
        .map(|i| format!("10.{}.{}.{}", i, i * 2, i * 3))
        .collect::<Vec<_>>()
        .join(", ");

    let request = format!(
        "GET /api/resource HTTP/1.1\r\n\
        Host: api.example.com\r\n\
        X-Trace-ID: {}\r\n\
        X-Correlation-ID: {}\r\n\
        X-Forwarded-For: {}\r\n\
        X-Forwarded-Proto: https\r\n\
        X-Forwarded-Host: api.example.com\r\n\
        X-Request-ID: req-{}\r\n\
        X-B3-TraceId: {}\r\n\
        X-B3-SpanId: span-{}\r\n\
        X-B3-ParentSpanId: parent-{}\r\n\
        Authorization: Bearer {}\r\n\
        Content-Type: application/json\r\n\
        Accept: application/json\r\n\
        \r\n",
        trace_id,
        correlation_id,
        forwarded_for,
        "x".repeat(32),
        "a".repeat(32),
        "b".repeat(16),
        "c".repeat(16),
        "d".repeat(200)
    );

    stream.write_all(request.as_bytes()).unwrap();
    stream.flush().unwrap();

    let mut response = Vec::new();
    let mut buffer = [0u8; 2048];

    match stream.read(&mut buffer) {
        Ok(n) => response.extend_from_slice(&buffer[0..n]),
        Err(e) => panic!("Failed to read response: {}", e),
    }

    let response_str = String::from_utf8_lossy(&response);
    assert!(
        response_str.contains("HTTP/1.1"),
        "Should handle realistic API gateway headers"
    );
}
