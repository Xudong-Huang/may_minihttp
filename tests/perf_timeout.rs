//! P2: Timeout behavior test — 7.8 from PERFORMANCE_AUDIT.md.
//!
//! Verifies that HttpClient::set_timeout() correctly triggers read/write timeouts
//! and that the connection is cleaned up afterward. The server deliberately delays
//! responses to exceed the client timeout window.
//!
//! Run with:
//!     cargo test --test perf_timeout --features client -- --test-threads=1 --nocapture

use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Once};
use std::thread;
use std::time::{Duration, Instant};

use may_minihttp::client::HttpClient;
use may_minihttp::{HttpServer, HttpService, Request as ServerRequest, Response as ServerResponse};

static INIT: Once = Once::new();

fn init_may_runtime() {
    INIT.call_once(|| {
        let _ = may::config().set_stack_size(0x8000);
    });
}

// ============================================================================
// Service: delays response by a configurable amount
// ============================================================================

struct DelayState {
    request_count: AtomicU64,
    delay_enabled: AtomicBool,
}

impl Clone for DelayState {
    fn clone(&self) -> Self {
        Self {
            request_count: AtomicU64::new(self.request_count.load(Ordering::Relaxed)),
            delay_enabled: AtomicBool::new(self.delay_enabled.load(Ordering::Relaxed)),
        }
    }
}

#[derive(Clone)]
struct DelayService {
    state: Arc<DelayState>,
}

impl HttpService for DelayService {
    fn call(&mut self, _req: ServerRequest, res: &mut ServerResponse) -> io::Result<()> {
        self.state.request_count.fetch_add(1, Ordering::Relaxed);

        if self.state.delay_enabled.load(Ordering::Relaxed) {
            // Delay long enough to exceed the 100ms client timeout
            thread::sleep(Duration::from_millis(500));
        }

        res.body("delayed ok");
        Ok(())
    }
}

// ============================================================================
// Fixture
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

struct TimeoutFixture {
    port: u16,
    shutdown: Arc<AtomicBool>,
    server_thread: Option<thread::JoinHandle<()>>,
    state: Arc<DelayState>,
}

impl TimeoutFixture {
    fn new(preferred_port: u16) -> Self {
        init_may_runtime();

        let port = find_available_port(preferred_port);
        let state = Arc::new(DelayState {
            request_count: AtomicU64::new(0),
            delay_enabled: AtomicBool::new(false),
        });
        let state_clone = Arc::clone(&state);
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = Arc::clone(&shutdown);
        let addr = format!("127.0.0.1:{port}");

        let svc = DelayService {
            state: Arc::clone(&state),
        };

        let server_thread = thread::spawn(move || {
            let handle = HttpServer(svc).start(&addr).expect("Failed to start");
            while !shutdown_clone.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(50));
            }
            eprintln!(
                "  [server] requests={}, delay_enabled={}",
                state_clone.request_count.load(Ordering::Relaxed),
                state_clone.delay_enabled.load(Ordering::Relaxed),
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

impl Drop for TimeoutFixture {
    fn drop(&mut self) {
        self.state.delay_enabled.store(false, Ordering::Relaxed); // stop delaying so client unblocks
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(handle) = self.server_thread.take() {
            let _ = handle.join();
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

/// Verify that client timeout triggers when server delays response.
#[test]
fn test_read_timeout_triggers() {
    let fixture = TimeoutFixture::new(27000);
    let addr = fixture.base_url();

    eprintln!("\n=== Timeout: Read Timeout Triggers ===");

    // Enable server-side delay
    fixture.state.delay_enabled.store(true, Ordering::Relaxed);

    let mut client = HttpClient::connect(&*addr).expect("connect");
    client.set_timeout(Some(Duration::from_millis(100)));

    let start = Instant::now();
    let result = client.get("/".parse().expect("uri"));
    let elapsed = start.elapsed();

    // Must fail with timeout (would block/timed out error kind)
    assert!(result.is_err(), "Expected timeout error, but got success");
    let err = result.unwrap_err();
    assert!(
        matches!(
            err.kind(),
            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
        ),
        "Expected timed-out error, got kind={:?}: {}",
        err.kind(),
        err
    );

    eprintln!(
        "  Timeout triggered after {:.0}ms (target: 100ms)",
        elapsed.as_millis()
    );

    // Verify timeout is within expected window (100ms + 20% margin + overhead)
    assert!(
        elapsed >= Duration::from_millis(80),
        "Timeout fired too fast ({:.0}ms), likely didn't actually wait",
        elapsed.as_millis()
    );
    assert!(
        elapsed < Duration::from_millis(600),
        "Timeout took too long ({:.0}ms), server may not have been delaying",
        elapsed.as_millis()
    );
}

/// Verify client does NOT hang after a timeout — can make another request.
#[test]
fn test_timeout_then_recovery() {
    let fixture = TimeoutFixture::new(27100);
    let addr = fixture.base_url();

    eprintln!("\n=== Timeout: Recovery After Timeout ===");

    // First request with delay → should timeout
    fixture.state.delay_enabled.store(true, Ordering::Relaxed);

    let mut client = HttpClient::connect(&*addr).expect("connect");
    client.set_timeout(Some(Duration::from_millis(100)));

    let result = client.get("/".parse().expect("uri"));
    assert!(result.is_err(), "First request should timeout");

    // Disable delay so next request succeeds
    fixture.state.delay_enabled.store(false, Ordering::Relaxed);

    // Reconnect and verify normal operation
    let mut client2 = HttpClient::connect(&*addr).expect("connect");
    let resp = client2
        .get("/".parse().expect("uri"))
        .expect("Second request should succeed");
    let status = resp.status().as_u16();
    assert_eq!(status, 200);

    eprintln!("  Timeout → reconnect → success: OK");
}

/// Verify write timeout triggers when server is slow to read.
#[test]
fn test_write_timeout() {
    let fixture = TimeoutFixture::new(27200);
    let addr = fixture.base_url();

    eprintln!("\n=== Timeout: Write Timeout ===");

    // Enable delay — server won't read body quickly
    fixture.state.delay_enabled.store(true, Ordering::Relaxed);

    let mut client = HttpClient::connect(&*addr).expect("connect");
    client.set_timeout(Some(Duration::from_millis(100)));

    let body = vec![b'a'; 1000];
    let result = client.post("/".parse().expect("uri"), body.as_slice());

    // Should timeout (either write or read)
    assert!(result.is_err(), "Expected timeout on POST with slow server");

    let err = result.unwrap_err();
    assert!(
        matches!(
            err.kind(),
            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
        ),
        "Expected timeout error, got kind={}: {}",
        err.kind(),
        err
    );

    eprintln!("  Write/read timeout on POST: OK");
}

/// Verify zero timeout (disabled) does NOT error on normal operation.
#[test]
fn test_zero_timeout_no_false_error() {
    let fixture = TimeoutFixture::new(27300);
    let addr = fixture.base_url();

    eprintln!("\n=== Timeout: No False Timeout on Normal ===");

    // Delay disabled by default
    fixture.state.delay_enabled.store(false, Ordering::Relaxed);

    let mut client = HttpClient::connect(&*addr).expect("connect");
    client.set_timeout(Some(Duration::from_millis(0))); // zero = disabled

    let resp = client
        .get("/".parse().expect("uri"))
        .expect("should succeed");
    assert_eq!(resp.status().as_u16(), 200);

    eprintln!("  Zero timeout, no delay: success");
}
