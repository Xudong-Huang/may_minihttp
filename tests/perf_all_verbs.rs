//! P1: All HTTP verbs E2E test.
//!
//! The client_integration.rs unit tests verify wire format for each verb, but
//! there's no end-to-end test using a real server echo for PUT, DELETE, PATCH,
//! OPTIONS. This file tests all verbs through a real may_minihttp server.
//!
//! Run with:
//!     cargo test --test perf_all_verbs --features client -- --test-threads=1 --nocapture

use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Once};
use std::thread;
use std::time::Duration;

use http::Method;
use may_minihttp::client::HttpClient;
use may_minihttp::{HttpServer, HttpService, Request as ServerRequest, Response as ServerResponse};

static INIT: Once = Once::new();

fn init_may_runtime() {
    INIT.call_once(|| {
        let _ = may::config().set_stack_size(0x8000);
    });
}

// ============================================================================
// Service that records verb and echoes body
// ============================================================================

struct VerbState {
    get_count: AtomicU64,
    post_count: AtomicU64,
    put_count: AtomicU64,
    delete_count: AtomicU64,
    patch_count: AtomicU64,
    head_count: AtomicU64,
    options_count: AtomicU64,
    first_request: AtomicBool,
}

impl Clone for VerbState {
    fn clone(&self) -> Self {
        Self {
            get_count: AtomicU64::new(self.get_count.load(Ordering::Relaxed)),
            post_count: AtomicU64::new(self.post_count.load(Ordering::Relaxed)),
            put_count: AtomicU64::new(self.put_count.load(Ordering::Relaxed)),
            delete_count: AtomicU64::new(self.delete_count.load(Ordering::Relaxed)),
            patch_count: AtomicU64::new(self.patch_count.load(Ordering::Relaxed)),
            head_count: AtomicU64::new(self.head_count.load(Ordering::Relaxed)),
            options_count: AtomicU64::new(self.options_count.load(Ordering::Relaxed)),
            first_request: AtomicBool::new(self.first_request.load(Ordering::Relaxed)),
        }
    }
}

#[derive(Clone)]
struct VerbService {
    state: Arc<VerbState>,
}

impl HttpService for VerbService {
    fn call(&mut self, req: ServerRequest, res: &mut ServerResponse) -> io::Result<()> {
        // Skip the check_ready probe — it's the very first request
        if self.state.first_request.swap(false, Ordering::Relaxed) {
            res.body("ok");
            return Ok(());
        }

        match req.method() {
            "GET" => {
                let _ = self.state.get_count.fetch_add(1, Ordering::Relaxed);
            }
            "POST" => {
                let _ = self.state.post_count.fetch_add(1, Ordering::Relaxed);
            }
            "PUT" => {
                let _ = self.state.put_count.fetch_add(1, Ordering::Relaxed);
            }
            "DELETE" => {
                let _ = self.state.delete_count.fetch_add(1, Ordering::Relaxed);
            }
            "PATCH" => {
                let _ = self.state.patch_count.fetch_add(1, Ordering::Relaxed);
            }
            "HEAD" => {
                let _ = self.state.head_count.fetch_add(1, Ordering::Relaxed);
            }
            "OPTIONS" => {
                let _ = self.state.options_count.fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        }

        let mut body = Vec::new();
        let _ = req.body().read_to_end(&mut body);

        if body.is_empty() {
            res.body("ok");
        } else {
            res.body_mut().extend_from_slice(&body);
        }

        Ok(())
    }
}

// ============================================================================
// Test fixture
// ============================================================================

fn find_available_port(preferred: u16) -> u16 {
    for port in preferred..(preferred + 1000) {
        if TcpListener::bind(format!("127.0.0.1:{port}")).is_ok() {
            return port;
        }
    }
    panic!("No available port in range {preferred}");
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
        thread::sleep(Duration::from_millis(50));
    }
    false
}

struct VerbFixture {
    port: u16,
    shutdown: Arc<AtomicBool>,
    server_thread: Option<thread::JoinHandle<()>>,
    state: Arc<VerbState>,
}

impl VerbFixture {
    fn new(preferred_port: u16) -> Self {
        init_may_runtime();

        let port = find_available_port(preferred_port);
        let state = Arc::new(VerbState {
            get_count: AtomicU64::new(0),
            post_count: AtomicU64::new(0),
            put_count: AtomicU64::new(0),
            delete_count: AtomicU64::new(0),
            patch_count: AtomicU64::new(0),
            head_count: AtomicU64::new(0),
            options_count: AtomicU64::new(0),
            first_request: AtomicBool::new(true),
        });
        let state_clone = Arc::clone(&state);
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = Arc::clone(&shutdown);
        let addr = format!("127.0.0.1:{port}");

        let svc = VerbService {
            state: Arc::clone(&state),
        };

        let server_thread = thread::spawn(move || {
            let handle = HttpServer(svc).start(&addr).expect("Failed to start");
            while !shutdown_clone.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(50));
            }
            eprintln!(
                "  [server] GET={}, POST={}, PUT={}, DELETE={}, PATCH={}, HEAD={}, OPTIONS={}",
                state_clone.get_count.load(Ordering::Relaxed),
                state_clone.post_count.load(Ordering::Relaxed),
                state_clone.put_count.load(Ordering::Relaxed),
                state_clone.delete_count.load(Ordering::Relaxed),
                state_clone.patch_count.load(Ordering::Relaxed),
                state_clone.head_count.load(Ordering::Relaxed),
                state_clone.options_count.load(Ordering::Relaxed),
            );
            unsafe {
                handle.coroutine().cancel();
            }
            let _ = handle.join();
        });

        assert!(
            check_ready(port, 100),
            "Server failed to start on port {port}"
        );

        Self {
            port,
            shutdown,
            server_thread: Some(server_thread),
            state,
        }
    }

    fn base_url(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }
}

impl Drop for VerbFixture {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(handle) = self.server_thread.take() {
            let _ = handle.join();
        }
    }
}

// ============================================================================
// Helpers
// ============================================================================

fn read_all(response: &mut may_minihttp::client::Response) -> Vec<u8> {
    let mut buf = Vec::new();
    let _ = response.read_to_end(&mut buf);
    buf
}

/// Send a non-GET/POST verb and read the response body.
fn send_custom(
    client: &mut HttpClient,
    method: Method,
    uri: &str,
    body: Option<&[u8]>,
) -> io::Result<Vec<u8>> {
    let mut req = client.new_request(method, uri.parse().expect("uri"));
    if let Some(body) = body {
        req.send(body)?;
    }
    let mut response = client.send_request(req)?;
    let mut buf = Vec::new();
    let _ = response.read_to_end(&mut buf);
    Ok(buf)
}

// ============================================================================
// Tests
// ============================================================================

/// Test that all HTTP verbs work correctly via native HttpClient.
#[test]
fn test_all_http_verbs() {
    let fixture = VerbFixture::new(24000);
    let addr = fixture.base_url();

    eprintln!("\n=== All HTTP Verbs (E2E) ===");

    let body = b"test body data";

    // GET
    {
        let mut client = HttpClient::connect(&*addr).expect("connect");
        let mut resp = client.get("/".parse().expect("uri")).expect("GET");
        assert_eq!(&read_all(&mut resp), b"ok");
        assert_eq!(resp.status().as_u16(), 200);
        eprintln!("  GET: 200 OK");
    }

    // POST
    {
        let mut client = HttpClient::connect(&*addr).expect("connect");
        let mut resp = client
            .post("/".parse().expect("uri"), body.as_slice())
            .expect("POST");
        assert_eq!(read_all(&mut resp), body);
        assert_eq!(resp.status().as_u16(), 200);
        eprintln!("  POST: 200 OK (echoed {} bytes)", body.len());
    }

    // PUT
    {
        let mut client = HttpClient::connect(&*addr).expect("connect");
        let resp = send_custom(&mut client, Method::PUT, "/", Some(body)).expect("PUT");
        assert_eq!(resp, body);
        eprintln!("  PUT: 200 OK (echoed {} bytes)", body.len());
    }

    // DELETE
    {
        let mut client = HttpClient::connect(&*addr).expect("connect");
        let resp = send_custom(&mut client, Method::DELETE, "/", Some(body)).expect("DEL");
        assert_eq!(resp, body);
        eprintln!("  DELETE: 200 OK (echoed {} bytes)", body.len());
    }

    // PATCH
    {
        let mut client = HttpClient::connect(&*addr).expect("connect");
        let resp = send_custom(&mut client, Method::PATCH, "/", Some(body)).expect("PATCH");
        assert_eq!(resp, body);
        eprintln!("  PATCH: 200 OK (echoed {} bytes)", body.len());
    }

    // HEAD
    {
        let mut client = HttpClient::connect(&*addr).expect("connect");
        let mut req = client.new_request(Method::HEAD, "/".parse().expect("uri"));
        req.expect_body(false);
        let resp = client.send_request(req).expect("HEAD");
        assert_eq!(resp.status().as_u16(), 200);
        eprintln!("  HEAD: 200 OK (no body)");
    }

    // OPTIONS
    {
        let mut client = HttpClient::connect(&*addr).expect("connect");
        let resp = send_custom(&mut client, Method::OPTIONS, "/", Some(body)).expect("OPTIONS");
        assert_eq!(resp, body);
        eprintln!("  OPTIONS: 200 OK (echoed {} bytes)", body.len());
    }

    // Verify server counters
    assert_eq!(fixture.state.get_count.load(Ordering::Relaxed), 1);
    assert_eq!(fixture.state.post_count.load(Ordering::Relaxed), 1);
    assert_eq!(fixture.state.put_count.load(Ordering::Relaxed), 1);
    assert_eq!(fixture.state.delete_count.load(Ordering::Relaxed), 1);
    assert_eq!(fixture.state.patch_count.load(Ordering::Relaxed), 1);
    assert_eq!(fixture.state.head_count.load(Ordering::Relaxed), 1);
    assert_eq!(fixture.state.options_count.load(Ordering::Relaxed), 1);

    eprintln!("  Server counters: all verbs received exactly once");
}

/// Measure per-verb throughput.
#[test]
fn test_verb_throughput() {
    let fixture = VerbFixture::new(24100);
    let addr = fixture.base_url();
    let body = b"throughput test data";
    let iterations = 100;

    eprintln!("\n=== Per-Verb Throughput ({} iterations) ===", iterations);

    for method_name in &["GET", "POST", "PUT", "DELETE", "PATCH"] {
        let start = std::time::Instant::now();
        for _ in 0..iterations {
            match *method_name {
                "GET" => {
                    let mut client = HttpClient::connect(&*addr).expect("connect");
                    let mut resp = client.get("/".parse().expect("uri")).expect("GET");
                    let _ = read_all(&mut resp);
                }
                "POST" => {
                    let mut client = HttpClient::connect(&*addr).expect("connect");
                    let mut resp = client
                        .post("/".parse().expect("uri"), body.as_slice())
                        .expect("POST");
                    let _ = read_all(&mut resp);
                }
                "PUT" => {
                    let mut client = HttpClient::connect(&*addr).expect("connect");
                    let _ = send_custom(&mut client, Method::PUT, "/", Some(body));
                }
                "DELETE" => {
                    let mut client = HttpClient::connect(&*addr).expect("connect");
                    let _ = send_custom(&mut client, Method::DELETE, "/", Some(body));
                }
                "PATCH" => {
                    let mut client = HttpClient::connect(&*addr).expect("connect");
                    let _ = send_custom(&mut client, Method::PATCH, "/", Some(body));
                }
                _ => unreachable!(),
            }
        }
        let total = start.elapsed();
        let throughput = (iterations as f64) / total.as_secs_f64();
        eprintln!("  {}: {:.0} req/s", method_name, throughput);
    }
}
