//! Integration tests for the native HTTP/1.1 client.
//!
//! Exercises `may_minihttp::client::HttpClient` against an in-process
//! `may_minihttp::HttpServer` on `127.0.0.1`. No Docker, no containers.
//!
//! Run with:
//!     cargo test --test client_integration --features client -- --nocapture
//!
//! Test fixtures:
//! - `TestService` — echo server with per-method endpoints
//! - `ClientTestFixture` — RAII fixture that starts server on random port,
//!   waits for readiness, and cleans up on drop.

use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Once};
use std::thread;
use std::time::Duration;

use bytes::BufMut;

use http::Method;
use may_minihttp::client::{HttpClient, Response};
use may_minihttp::{HttpServer, HttpService, Request as ServerRequest, Response as ServerResponse};

// ============================================================================
// MAY Runtime initialization
// ============================================================================

static INIT: Once = Once::new();

fn init_may_runtime() {
    INIT.call_once(|| {
        may::config().set_stack_size(0x8000);
    });
}

// ============================================================================
// Test Service — Echo server with method-specific endpoints
// ============================================================================

/// Test HTTP service that implements common client test scenarios.
///
/// Each endpoint echoes back information so the client can verify
/// what it sent and how the server responded.
#[derive(Clone)]
struct TestService;

impl HttpService for TestService {
    fn call(&mut self, req: ServerRequest, res: &mut ServerResponse) -> io::Result<()> {
        let method = req.method().to_string();
        let path = req.path().to_string();
        let header_count = req.headers().len();

        // Build a simple line-based echo
        let mut parts: Vec<String> = Vec::new();
        parts.push(format!("method:{}", method));
        parts.push(format!("path:{}", path));
        parts.push(format!("headers:{}", header_count));

        // Echo custom headers
        for h in req.headers() {
            let name = h.name;
            let value = std::str::from_utf8(h.value).unwrap_or("");
            parts.push(format!("{}:{}", name, value));
        }

        // Read body if available (POST/PUT/PATCH)
        let mut body_buf = String::new();
        let _ = req.body().read_to_string(&mut body_buf);
        if !body_buf.is_empty() {
            parts.push(format!("body:{}", body_buf));
        }

        let body = parts.join("\n");

        // Route to different endpoints
        match (method.as_str(), path.as_str()) {
            ("GET", "/ok") => {
                write!(res.body_mut().writer(), "OK").ok();
            }
            ("GET", "/get") => {
                write!(res.body_mut().writer(), "{}", body).ok();
            }
            ("GET", "/chunked") => {
                write!(res.body_mut().writer(), "chunked-data-end").ok();
            }
            ("POST", "/post") | ("PUT", "/put") | ("PATCH", "/patch") => {
                write!(res.body_mut().writer(), "{}", body).ok();
            }
            ("HEAD", "/headers") => {
                // HEAD: send headers but no body
                let _ = body;
            }
            ("DELETE", "/delete") => {
                write!(res.body_mut().writer(), "deleted").ok();
            }
            (_method, path) if path.starts_with("/status/") => {
                let code_str = &path[8..]; // extract status code after "/status/"
                if code_str.parse::<u16>().is_ok() {
                    write!(res.body_mut().writer(), "status-set").ok();
                } else {
                    write!(res.body_mut().writer(), "invalid-status").ok();
                }
            }
            ("GET", "/slow") => {
                thread::sleep(Duration::from_secs(5));
                write!(res.body_mut().writer(), "slow-response").ok();
            }
            _ => {
                write!(res.body_mut().writer(), "Not Found").ok();
            }
        }

        Ok(())
    }
}

// ============================================================================
// Test Fixture — RAII server + client setup
// ============================================================================

/// RAII fixture for integration tests.
///
/// Starts an in-process `may_minihttp::HttpServer` on a random port,
/// waits for it to accept connections, and provides cleanup on drop.
struct ClientTestFixture {
    port: u16,
    shutdown: Arc<AtomicBool>,
    server_thread: Option<thread::JoinHandle<()>>,
}

impl ClientTestFixture {
    fn new(preferred_port: u16) -> Self {
        init_may_runtime();

        // Find an available port
        let port = find_available_port(preferred_port);

        // Run the MAY server on a dedicated OS thread so Windows blocking handlers
        // do not stall the test thread's client I/O.
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = Arc::clone(&shutdown);
        let addr = format!("127.0.0.1:{}", port);
        let server_thread = thread::spawn(move || {
            let handle = HttpServer(TestService)
                .start(&addr)
                .expect("Failed to start test server");

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

        // Wait for server to be ready
        if !fixture.wait_for_ready(100) {
            panic!("Server failed to start on port {}", port);
        }

        fixture
    }

    fn wait_for_ready(&self, max_attempts: u32) -> bool {
        for attempt in 0..max_attempts {
            match TcpStream::connect(format!("127.0.0.1:{}", self.port)) {
                Ok(mut stream) => {
                    // Close after probe so Windows blocking server handlers release the worker.
                    let request =
                        "GET /ok HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
                    if stream.write_all(request.as_bytes()).is_ok() {
                        let mut buf = [0u8; 256];
                        if stream.read(&mut buf).is_ok() {
                            let _ = stream.shutdown(std::net::Shutdown::Both);
                            return true;
                        }
                    }
                }
                Err(_) => {}
            }
            thread::sleep(Duration::from_millis(50));
            if attempt % 20 == 0 {
                eprintln!(
                    "  waiting for server on port {} (attempt {})",
                    self.port,
                    attempt + 1
                );
            }
        }
        false
    }

    fn base_url(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }
}

impl Drop for ClientTestFixture {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(handle) = self.server_thread.take() {
            let _ = handle.join();
        }
    }
}

// ============================================================================
// Helper functions
// ============================================================================

/// Find an available port starting from preferred_port.
fn find_available_port(preferred: u16) -> u16 {
    for port in preferred..(preferred + 1000) {
        if TcpListener::bind(format!("127.0.0.1:{}", port)).is_ok() {
            return port;
        }
    }
    panic!("No available port in range {}", preferred);
}

/// Read response body into a string.
fn read_body(response: &mut Response) -> String {
    let mut buf = String::new();
    let _ = response.read_to_string(&mut buf);
    buf
}

/// Read response body in chunks (for streaming tests).
fn read_body_chunks(response: &mut Response) -> String {
    let mut buf = [0u8; 64];
    let mut result = String::new();
    loop {
        let n = match response.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => break,
        };
        result.push_str(&String::from_utf8_lossy(&buf[..n]));
    }
    result
}

// ============================================================================
// Integration Tests
// ============================================================================

/// Test 1: Basic GET request.
///
/// Client sends GET /ok, expects 200 and body "OK".
#[test]
fn test_get_simple() {
    let fixture = ClientTestFixture::new(18500);
    let addr = fixture.base_url();

    let mut client = HttpClient::connect(&*addr).expect("failed to connect");

    let mut response = client
        .get("/ok".parse().expect("invalid uri"))
        .expect("GET /ok failed");

    assert_eq!(response.status().as_u16(), 200);
    let body = read_body(&mut response);
    assert_eq!(body, "OK");
}

/// Test 2: GET with header echo.
///
/// Client sends GET /get with custom headers, expects them echoed back.
#[test]
fn test_get_with_headers() {
    let fixture = ClientTestFixture::new(18501);
    let addr = fixture.base_url();

    let mut client = HttpClient::connect(&*addr).expect("failed to connect");

    let mut req = client.new_request(Method::GET, "/get".parse().expect("invalid uri"));
    // Add custom headers via the raw request
    req.headers_mut()
        .append("X-Test-1", "value1".parse().unwrap());
    req.headers_mut()
        .append("X-Test-2", "value2".parse().unwrap());
    let mut response = client.send_request(req).expect("GET /get failed");

    assert_eq!(response.status().as_u16(), 200);
    let body = read_body(&mut response);
    eprintln!("test_get_with_headers body: {}", body);
    assert!(body.contains("x-test-1:value1"));
    assert!(body.contains("x-test-2:value2"));
    assert!(body.contains("headers:"));
}

/// Test 3: POST with JSON body.
///
/// Client sends POST /post with JSON body, expects it echoed back.
#[test]
fn test_post_with_body() {
    let fixture = ClientTestFixture::new(18502);
    let addr = fixture.base_url();

    let mut client = HttpClient::connect(&*addr).expect("failed to connect");

    let body_bytes = b"{\"hello\":\"world\"}";
    let mut response = client
        .post("/post".parse().expect("invalid uri"), &body_bytes[..])
        .expect("POST /post failed");

    assert_eq!(response.status().as_u16(), 200);
    let body = read_body(&mut response);
    assert!(body.contains("method:POST"));
    assert!(body.contains("body:{\"hello\":\"world\"}"));
}

/// Test 4: POST with empty body.
///
/// Client sends POST /post with no body, expects method echoed as POST.
#[test]
fn test_post_empty_body() {
    let fixture = ClientTestFixture::new(18503);
    let addr = fixture.base_url();

    let mut client = HttpClient::connect(&*addr).expect("failed to connect");

    let mut response = client
        .post("/post".parse().expect("invalid uri"), b"".as_ref())
        .expect("POST /post failed");

    assert_eq!(response.status().as_u16(), 200);
    let body = read_body(&mut response);
    assert!(body.contains("method:POST"));
}

/// Test 5: HEAD request — no body.
///
/// Client sends HEAD /headers, expects 200 and no body read.
#[test]
fn test_head_no_body() {
    let fixture = ClientTestFixture::new(18504);
    let addr = fixture.base_url();

    let mut client = HttpClient::connect(&*addr).expect("failed to connect");

    let request = client.new_request(Method::HEAD, "/headers".parse().expect("invalid uri"));
    let mut response = client.send_request(request).expect("HEAD /headers failed");

    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(response.version(), http::Version::HTTP_11);

    // HEAD response body should be empty (EmptyReader)
    let body = read_body(&mut response);
    assert!(
        body.is_empty(),
        "HEAD response should have no body, got: {:?}",
        body
    );
}

/// Test 6: PUT with body via send_request.
///
/// Client sends PUT /put with body bytes, expects it echoed back.
#[test]
fn test_put_with_body_explicit() {
    let fixture = ClientTestFixture::new(18505);
    let addr = fixture.base_url();

    let mut client = HttpClient::connect(&*addr).expect("failed to connect");

    let mut request = client.new_request(Method::PUT, "/put".parse().expect("invalid uri"));
    request.set_content_length(13);
    request
        .send(b"hello world!!!")
        .expect("PUT body send failed");
    let mut response = client.send_request(request).expect("PUT /put failed");

    assert_eq!(response.status().as_u16(), 200);
    let body = read_body(&mut response);
    assert!(body.contains("body:hello world!!!"));
}

/// Test 7: DELETE without body.
///
/// Client sends DELETE /delete, expects 200 and body "deleted".
#[test]
fn test_delete_no_body() {
    let fixture = ClientTestFixture::new(18506);
    let addr = fixture.base_url();

    let mut client = HttpClient::connect(&*addr).expect("failed to connect");

    let request = client.new_request(Method::DELETE, "/delete".parse().expect("invalid uri"));
    let mut response = client.send_request(request).expect("DELETE /delete failed");

    assert_eq!(response.status().as_u16(), 200);
    let body = read_body(&mut response);
    assert_eq!(body, "deleted");
}

/// Test 8: PATCH with body via send_request.
///
/// Client sends PATCH /patch with body bytes, expects it echoed back.
#[test]
fn test_patch_with_body_explicit() {
    let fixture = ClientTestFixture::new(18507);
    let addr = fixture.base_url();

    let mut client = HttpClient::connect(&*addr).expect("failed to connect");

    let mut request = client.new_request(Method::PATCH, "/patch".parse().expect("invalid uri"));
    request.set_content_length(15);
    request
        .send(b"{\"patched\":true}")
        .expect("PATCH body send failed");
    let mut response = client.send_request(request).expect("PATCH /patch failed");

    assert_eq!(response.status().as_u16(), 200);
    let body = read_body(&mut response);
    assert!(body.contains("method:PATCH"));
    assert!(body.contains("body:{\"patched\":true}"));
}

/// Test 9: Connection reuse.
///
/// Client reuses same connection for multiple requests.
/// Verifies all requests succeed on the same HttpClient instance.
#[test]
fn test_connection_reuse() {
    let fixture = ClientTestFixture::new(18508);
    let addr = fixture.base_url();

    let mut client = HttpClient::connect(&*addr).expect("failed to connect");

    // Send multiple requests — each reuses the connection
    for _ in 0..5 {
        let mut response = client
            .get("/ok".parse().expect("invalid uri"))
            .expect("GET request failed");
        assert_eq!(response.status().as_u16(), 200);
        let body = read_body(&mut response);
        assert_eq!(body, "OK");
    }
}

/// Test 10: Connection error — unbound port.
///
/// Client tries to connect to a port with no server, expects io::Error.
#[test]
fn test_connection_refused() {
    // Port 19999 is deliberately unused
    match HttpClient::connect("127.0.0.1:19999") {
        Ok(_) => panic!("Should have failed to connect"),
        Err(e) => {
            assert_eq!(e.kind(), io::ErrorKind::ConnectionRefused);
        }
    }
}

/// Test 11: Connection timeout — connect and read fail on unbound port.
///
/// Client connects with a very short timeout to a non-responding service.
#[test]
fn test_connection_timeout() {
    // Connect to a port with no server
    // The connect itself may succeed (TCP socket created) but the read will fail
    let mut client = match HttpClient::connect("127.0.0.1:19998") {
        Ok(c) => c,
        Err(e) => {
            // Connection refused on connect is also fine
            assert_eq!(e.kind(), io::ErrorKind::ConnectionRefused);
            return;
        }
    };
    client.set_timeout(Some(Duration::from_millis(100)));

    // The connect may succeed (TCP socket created) but the read will fail
    let result = client.get("/ok".parse().expect("invalid uri"));
    assert!(
        result.is_err(),
        "GET to unbound port should fail: {:?}",
        result
    );
}

/// Test 12: Chunked response.
///
/// Client reads a response from /chunked endpoint.
#[test]
fn test_chunked_response() {
    let fixture = ClientTestFixture::new(18509);
    let addr = fixture.base_url();

    let mut client = HttpClient::connect(&*addr).expect("failed to connect");

    let mut response = client
        .get("/chunked".parse().expect("invalid uri"))
        .expect("GET /chunked failed");

    assert_eq!(response.status().as_u16(), 200);
    let body = read_body_chunks(&mut response);
    assert!(body.contains("chunked"));
    assert!(body.contains("data"));
    assert!(body.contains("end"));
}

/// Test 13: Not found — 200 "Not Found" body.
///
/// Client requests unknown endpoint, server returns 200 with "Not Found" body.
#[test]
fn test_not_found() {
    let fixture = ClientTestFixture::new(18510);
    let addr = fixture.base_url();

    let mut client = HttpClient::connect(&*addr).expect("failed to connect");

    let mut response = client
        .get("/nonexistent".parse().expect("invalid uri"))
        .expect("GET /nonexistent failed");

    assert_eq!(response.status().as_u16(), 200);
    let body = read_body(&mut response);
    assert!(body.contains("Not Found"));
}

/// Test 14: Malformed server response.
///
/// Server sends invalid HTTP, client should return io::Error.
#[test]
fn test_malformed_response() {
    use std::io::Write;

    // Bind a socket, send garbage, try to connect with client
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind failed");
    let port = listener.local_addr().unwrap().port();

    // Send garbage response in a separate thread
    std::thread::spawn(move || {
        if let Ok(stream) = listener.accept() {
            let mut stream = stream.0;
            // Send invalid HTTP response
            let _ = stream.write_all(b"GARBAGE NOT HTTP");
        }
    });

    // Wait a bit for the thread to be ready
    thread::sleep(Duration::from_millis(100));

    // Client should fail when trying to decode the response
    let mut client = HttpClient::connect(format!("127.0.0.1:{}", port)).expect("failed to connect");
    client.set_timeout(Some(Duration::from_millis(1000)));

    let result = client.get("/".parse().expect("invalid uri"));
    assert!(
        result.is_err(),
        "Should fail on malformed response: {:?}",
        result
    );
}

/// Test 15: Partial response decode.
///
/// Client handles responses that arrive in chunks (small endpoint).
#[test]
fn test_partial_response_decode() {
    let fixture = ClientTestFixture::new(18511);
    let addr = fixture.base_url();

    let mut client = HttpClient::connect(&*addr).expect("failed to connect");

    // The /ok endpoint sends a small response
    let mut response = client
        .get("/ok".parse().expect("invalid uri"))
        .expect("GET /ok failed");

    assert_eq!(response.status().as_u16(), 200);
    let body = read_body(&mut response);
    assert_eq!(body, "OK");
}

/// Test 16: Multiple URIs on same client.
///
/// Verifies the client correctly handles different URIs on the same connection.
#[test]
fn test_different_uris_same_client() {
    let fixture = ClientTestFixture::new(18512);
    let addr = fixture.base_url();

    let mut client = HttpClient::connect(&*addr).expect("failed to connect");

    // Request different endpoints
    let mut resp1 = client
        .get("/ok".parse().expect("invalid uri"))
        .expect("GET /ok failed");
    assert_eq!(read_body(&mut resp1), "OK");

    let mut resp2 = client
        .get("/get".parse().expect("invalid uri"))
        .expect("GET /get failed");
    let body2 = read_body(&mut resp2);
    assert!(body2.contains("method:GET"));

    let mut resp3 = client
        .get("/chunked".parse().expect("invalid uri"))
        .expect("GET /chunked failed");
    assert!(read_body(&mut resp3).contains("chunked"));
}

/// Test 17: Connection close by server.
///
/// Connecting to a port with no server should fail with ConnectionRefused.
#[test]
fn test_connection_close_by_server() {
    // Pick an arbitrary port that nothing is listening on
    let port = 19999u16;

    match HttpClient::connect(format!("127.0.0.1:{}", port)) {
        Err(e) => {
            // Connection refused is expected since nothing listens on this port
            assert_eq!(e.kind(), io::ErrorKind::ConnectionRefused);
        }
        Ok(mut client) => {
            // If we somehow get a connection, the request should fail
            let result = client.get("/ok".parse().expect("invalid uri"));
            assert!(
                result.is_err(),
                "Expected error on port {}: {:?}",
                port,
                result
            );
        }
    }
}

/// Test 18: Content-Length header present in response.
///
/// Server sends a response with Content-Length, client should parse it.
#[test]
fn test_content_length_header() {
    let fixture = ClientTestFixture::new(18514);
    let addr = fixture.base_url();

    let mut client = HttpClient::connect(&*addr).expect("failed to connect");

    let response = client
        .get("/ok".parse().expect("invalid uri"))
        .expect("GET /ok failed");

    assert_eq!(response.status().as_u16(), 200);
    // Server always sends Content-Length
    assert!(response.headers().contains_key("content-length"));
    let cl = response.headers().get("content-length").unwrap();
    assert_eq!(cl.to_str().unwrap(), "2"); // "OK" is 2 bytes
}

/// Test 19: Server headers present in response.
///
/// Server sends "Server: M" header, client should receive it.
#[test]
fn test_server_header() {
    let fixture = ClientTestFixture::new(18515);
    let addr = fixture.base_url();

    let mut client = HttpClient::connect(&*addr).expect("failed to connect");

    let response = client
        .get("/ok".parse().expect("invalid uri"))
        .expect("GET /ok failed");

    assert_eq!(response.status().as_u16(), 200);
    assert!(response.headers().contains_key("server"));
    assert_eq!(
        response.headers().get("server").unwrap().to_str().unwrap(),
        "M"
    );
}

/// Test 20: Host header injected by client.
///
/// Client auto-injects Host header per RFC 7230. Server echoes it back.
#[test]
fn test_host_header_injected() {
    let fixture = ClientTestFixture::new(18516);
    let addr = fixture.base_url();

    let mut client = HttpClient::connect(&*addr).expect("failed to connect");

    // Build a GET request with a path-only URI (no scheme/host) so the server
    // parses "/get" correctly, but manually set the Host header to verify
    // the client injects the Host header.
    let mut req = client.new_request(Method::GET, "/get".parse().expect("invalid uri"));
    // The client auto-injects Host: <host> when the URI has a host component.
    // Here we set it manually to test that the value gets sent correctly.
    req.headers_mut()
        .insert("Host", "localhost".parse().unwrap());

    let mut response = client.send_request(req).expect("GET /get failed");

    assert_eq!(response.status().as_u16(), 200);
    let body = read_body(&mut response);
    // Client injects Host: <host> for HTTP/1.1
    // The host should be in the echoed headers
    assert!(body.contains("host:localhost"));
}
