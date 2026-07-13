//! P1: Keep-alive / connection reuse test.
//!
//! The server's `each_connection_loop` is a loop — it processes multiple requests
//! per TCP connection. The client shares an `Rc<RefCell<BufferIo<TcpStream>>>` across
//! requests via `new_request()` + `send_request()`, enabling connection reuse.
//! This tests that a single connection can handle many sequential requests correctly
//! with no data leakage between requests.
//!
//! Run with:
//!     cargo test --test perf_keepalive --features client -- --test-threads=1 --nocapture

use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Once};
use std::thread;
use std::time::{Duration, Instant};

use http::Method;
use may_minihttp::client::{HttpClient, Request};
use may_minihttp::{HttpServer, HttpService, Request as ServerRequest, Response as ServerResponse};

static INIT: Once = Once::new();

fn init_may_runtime() {
    INIT.call_once(|| {
        let _ = may::config().set_stack_size(0x8000);
    });
}

// ============================================================================
// Service that echoes body with a request counter prefix
// ============================================================================

#[derive(Clone)]
struct KeepAliveService {
    state: Arc<KeepAliveState>,
}

/// State shared between service and test. The `first_request` flag tracks the
/// probe sent by `check_ready()` so it doesn't pollute test counters.
struct KeepAliveState {
    request_count: AtomicU64,
    total_body_bytes: AtomicU64,
    first_request: AtomicBool,
}

impl Clone for KeepAliveState {
    fn clone(&self) -> Self {
        Self {
            request_count: AtomicU64::new(self.request_count.load(Ordering::Relaxed)),
            total_body_bytes: AtomicU64::new(self.total_body_bytes.load(Ordering::Relaxed)),
            first_request: AtomicBool::new(self.first_request.load(Ordering::Relaxed)),
        }
    }
}

impl HttpService for KeepAliveService {
    fn call(&mut self, req: ServerRequest, res: &mut ServerResponse) -> io::Result<()> {
        // Skip the check_ready probe — it's the very first request
        let is_probe = self.state.first_request.swap(false, Ordering::Relaxed);
        let n = if is_probe {
            0 // probe doesn't count
        } else {
            self.state.request_count.fetch_add(1, Ordering::Relaxed) + 1
        };

        // Echo body with counter prefix so we can verify order
        let mut body = Vec::new();
        let _ = req.body().read_to_end(&mut body);

        if body.is_empty() {
            // For GET: just echo the counter
            let prefix = format!("seq:{}\n", n);
            res.body_mut().extend_from_slice(prefix.as_bytes());
        } else {
            // For body methods: prepend counter, then echo
            let prefix = format!("seq:{}\n", n);
            res.body_mut().extend_from_slice(prefix.as_bytes());
            res.body_mut().extend_from_slice(&body);
        }

        self.state
            .total_body_bytes
            .fetch_add(body.len() as u64, Ordering::Relaxed);
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

struct KeepAliveFixture {
    port: u16,
    shutdown: Arc<AtomicBool>,
    server_thread: Option<thread::JoinHandle<()>>,
    state: Arc<KeepAliveState>,
}

impl KeepAliveFixture {
    fn new(preferred_port: u16) -> Self {
        init_may_runtime();

        let port = find_available_port(preferred_port);
        let state = Arc::new(KeepAliveState {
            request_count: AtomicU64::new(0),
            total_body_bytes: AtomicU64::new(0),
            first_request: AtomicBool::new(true),
        });
        let state_clone = Arc::clone(&state);
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = Arc::clone(&shutdown);
        let addr = format!("127.0.0.1:{port}");

        let svc = KeepAliveService {
            state: Arc::clone(&state),
        };

        let server_thread = thread::spawn(move || {
            let handle = HttpServer(svc)
                .start(&addr)
                .expect("Failed to start test server");
            while !shutdown_clone.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(50));
            }
            eprintln!(
                "  [server] requests={}, body_bytes={}",
                state_clone.request_count.load(Ordering::Relaxed),
                state_clone.total_body_bytes.load(Ordering::Relaxed)
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

impl Drop for KeepAliveFixture {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(handle) = self.server_thread.take() {
            let _ = handle.join();
        }
    }
}

fn read_all(response: &mut may_minihttp::client::Response) -> Vec<u8> {
    let mut buf = Vec::new();
    let _ = response.read_to_end(&mut buf);
    buf
}

// ============================================================================
// Tests
// ============================================================================

/// Test that a single HttpClient instance handles sequential requests
/// on one TCP connection with correct response routing.
#[test]
fn test_keepalive_sequential_requests() {
    let fixture = KeepAliveFixture::new(25000);
    let addr = fixture.base_url();

    eprintln!("\n=== Keep-Alive: Sequential Requests (single connection) ===");

    // Send 50 sequential GETs on ONE connection
    let mut client = HttpClient::connect(&*addr).expect("connect");
    let mut expected_seq = 1u64;

    for _ in 0..50 {
        let resp = client.get("/".parse().expect("uri")).expect("GET");
        let mut resp_body = resp;
        let body = read_all(&mut resp_body);
        let line = String::from_utf8_lossy(&body);

        // Response is "seq:{n}\n" — verify counter increments
        assert!(
            line.starts_with(&format!("seq:{expected_seq}\n")),
            "Request #{}: expected 'seq:{}\\n', got {:?}",
            expected_seq,
            expected_seq,
            line
        );
        expected_seq += 1;
    }

    assert_eq!(expected_seq, 51, "Expected 50 requests processed");

    // Verify server state
    let req_count = fixture.state.request_count.load(Ordering::Relaxed);
    assert_eq!(req_count, 50, "Server received 50 requests");

    eprintln!("  50 sequential requests on 1 connection: OK");
    eprintln!("  Server counter: {}", req_count);
}

/// Test POST on reused connection — body integrity across requests.
#[test]
fn test_keepalive_post_body_integrity() {
    let fixture = KeepAliveFixture::new(25100);
    let addr = fixture.base_url();

    eprintln!("\n=== Keep-Alive: POST Body Integrity ===");

    let mut client = HttpClient::connect(&*addr).expect("connect");

    for i in 1..=20 {
        let body = format!("request-{i}").into_bytes();
        let expected_response = format!("seq:{i}\n").into_bytes();

        let mut resp = client
            .post("/".parse().expect("uri"), body.as_slice())
            .expect("POST");
        let resp_body = read_all(&mut resp);

        // Response = "seq:{i}\n" + echo of body
        assert!(
            resp_body.starts_with(&expected_response),
            "POST #{}: response should start with seq:{}",
            i,
            i
        );
        assert!(
            resp_body.ends_with(&body),
            "POST #{}: response should echo body",
            i
        );
    }

    let req_count = fixture.state.request_count.load(Ordering::Relaxed);
    assert_eq!(req_count, 20, "Server received 20 POST requests");
    eprintln!("  20 POSTs on 1 connection: OK");
}

/// Test that new connection vs reused connection has measurable difference.
#[test]
fn test_keepalive_overhead_comparison() {
    let fixture = KeepAliveFixture::new(25200);
    let addr = fixture.base_url();
    let iterations = 100;

    eprintln!("\n=== Keep-Alive: Connection Overhead Comparison ===");

    // Method A: each request gets a fresh connection
    eprintln!("  --- Fresh connections ---");
    let start = Instant::now();
    for _ in 0..iterations {
        let mut client = HttpClient::connect(&*addr).expect("connect");
        let mut resp = client.get("/".parse().expect("uri")).expect("GET");
        let _ = read_all(&mut resp);
    }
    let fresh_time = start.elapsed();

    // Method B: all requests on one connection
    eprintln!("  --- Reused connection ---");
    let mut client = HttpClient::connect(&*addr).expect("connect");
    let start = Instant::now();
    for _ in 0..iterations {
        let mut resp = client.get("/".parse().expect("uri")).expect("GET");
        let _ = read_all(&mut resp);
    }
    let reused_time = start.elapsed();

    let fresh_reqs = (iterations as f64) / fresh_time.as_secs_f64();
    let reused_reqs = (iterations as f64) / reused_time.as_secs_f64();

    eprintln!(
        "  Fresh connections:    {:.0} req/s (total: {:?})",
        fresh_reqs, fresh_time
    );
    eprintln!(
        "  Reused connection:    {:.0} req/s (total: {:?})",
        reused_reqs, reused_time
    );
    eprintln!("  Speedup:              {:.1}x", reused_reqs / fresh_reqs);

    // Reused should be measurably faster (at least 20% improvement)
    assert!(
        reused_reqs > fresh_reqs * 1.2,
        "Reused connection should be faster: fresh={:.0} reused={:.0}",
        fresh_reqs,
        reused_reqs
    );
}

/// Test mixed GET/POST on a reused connection.
#[test]
fn test_keepalive_mixed_methods() {
    let fixture = KeepAliveFixture::new(25300);
    let addr = fixture.base_url();

    eprintln!("\n=== Keep-Alive: Mixed GET/POST ===");

    let mut client = HttpClient::connect(&*addr).expect("connect");

    for i in 1..=30 {
        if i % 3 == 0 {
            // POST every third request
            let body = format!("post-{i}");
            let body_bytes = body.as_bytes();
            let mut resp = client
                .post("/".parse().expect("uri"), body_bytes)
                .expect("POST");
            let resp_body = read_all(&mut resp);
            assert!(
                resp_body.starts_with(&format!("seq:{i}\n").into_bytes()),
                "POST #{} counter mismatch",
                i
            );
        } else {
            // GET on other requests
            let resp = client.get("/".parse().expect("uri")).expect("GET");
            let mut resp_body = resp;
            let body = read_all(&mut resp_body);
            assert!(
                body.starts_with(&format!("seq:{i}\n").into_bytes()),
                "GET #{} counter mismatch, got {:?}",
                i,
                String::from_utf8_lossy(&body)
            );
        }
    }

    let req_count = fixture.state.request_count.load(Ordering::Relaxed);
    assert_eq!(req_count, 30);
    eprintln!("  30 mixed GET/POST on 1 connection: OK");
}
