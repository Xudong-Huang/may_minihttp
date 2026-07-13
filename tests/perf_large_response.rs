//! Performance tests: large response body reads.
//!
//! Measures the client's ability to read large responses from the server.
//! Also measures server response encoding throughput for large bodies.
//!
//! Run with:
//!     cargo test --test perf_large_response --features client -- --test-threads=1 --nocapture

use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Once};
use std::thread;
use std::time::{Duration, Instant};

use bytes::BytesMut;
use may_minihttp::client::Response;
use may_minihttp::{HttpServer, HttpService, Request, Response as ServerResponse};

// ============================================================================
// Runtime Init
// ============================================================================

static INIT: Once = Once::new();

fn init_may_runtime() {
    INIT.call_once(|| {
        may::config().set_stack_size(0x8000);
    });
}

// ============================================================================
// Echo service with configurable response size
// ============================================================================

struct ServiceState {
    request_count: Arc<AtomicU64>,
    total_bytes_written: Arc<AtomicU64>,
}

impl Clone for ServiceState {
    fn clone(&self) -> Self {
        Self {
            request_count: Arc::clone(&self.request_count),
            total_bytes_written: Arc::clone(&self.total_bytes_written),
        }
    }
}

impl Default for ServiceState {
    fn default() -> Self {
        Self {
            request_count: Arc::new(AtomicU64::new(0)),
            total_bytes_written: Arc::new(AtomicU64::new(0)),
        }
    }
}

#[derive(Clone)]
struct LargeResponseService {
    state: Arc<ServiceState>,
    fixed_size: usize,
}

impl HttpService for LargeResponseService {
    fn call(&mut self, _req: Request, res: &mut ServerResponse) -> io::Result<()> {
        self.state.request_count.fetch_add(1, Ordering::Relaxed);
        let size = self.fixed_size;

        // Allocate body directly into the response buffer (no heap copy)
        let body_buf = res.body_mut();
        body_buf.reserve(size);

        // Fill with repeating pattern for integrity verification
        let pattern = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
        let mut remaining = size;
        while remaining > 0 {
            let chunk = remaining.min(pattern.len());
            body_buf.extend_from_slice(&pattern[..chunk]);
            remaining -= chunk;
        }

        self.state
            .total_bytes_written
            .fetch_add(size as u64, Ordering::Relaxed);
        Ok(())
    }
}

// ============================================================================
// Test fixture
// ============================================================================

fn find_available_port(preferred: u16) -> u16 {
    for port in preferred..(preferred + 1000) {
        if TcpListener::bind(format!("127.0.0.1:{}", port)).is_ok() {
            return port;
        }
    }
    panic!("No available port in range {}", preferred);
}

fn check_ready(port: u16, max_attempts: u32) -> bool {
    for _ in 0..max_attempts {
        match TcpStream::connect(format!("127.0.0.1:{}", port)) {
            Ok(mut stream) => {
                let req = "GET /ok HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
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

struct LargeResponseFixture {
    port: u16,
    shutdown: Arc<AtomicBool>,
    server_thread: Option<thread::JoinHandle<()>>,
    state: Arc<ServiceState>,
}

impl LargeResponseFixture {
    fn new(preferred_port: u16, fixed_size: usize) -> Self {
        init_may_runtime();

        let port = find_available_port(preferred_port);
        let state = Arc::new(ServiceState::default());
        let state_clone = Arc::clone(&state);
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = Arc::clone(&shutdown);
        let addr = format!("127.0.0.1:{}", port);

        let svc = LargeResponseService {
            state: Arc::clone(&state),
            fixed_size,
        };

        let server_thread = thread::spawn(move || {
            let handle = HttpServer(svc)
                .start(&addr)
                .expect("Failed to start test server");
            while !shutdown_clone.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(50));
            }
            eprintln!(
                "  [server] requests={}, bytes_written={}",
                state_clone.request_count.load(Ordering::Relaxed),
                state_clone.total_bytes_written.load(Ordering::Relaxed)
            );
            unsafe {
                handle.coroutine().cancel();
            }
            let _ = handle.join();
        });

        assert!(
            check_ready(port, 100),
            "Server failed to start on port {}",
            port
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

impl Drop for LargeResponseFixture {
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

fn read_all_body(response: &mut Response) -> Vec<u8> {
    let mut buf = Vec::new();
    let _ = response.read_to_end(&mut buf);
    buf
}

// ============================================================================
// Tests: Large response body reads
// ============================================================================

/// Test that the client can read various response sizes correctly.
#[test]
fn test_large_response_body_sizes() {
    let sizes = [100, 1_000, 10_000, 100_000];

    eprintln!("\n=== Large Response Body Sizes ===");

    for size in &sizes {
        let fixture = LargeResponseFixture::new(22000, *size);
        let addr = fixture.base_url();

        let start = Instant::now();
        let mut client = may_minihttp::client::HttpClient::connect(&*addr).expect("connect");
        let mut response = client.get("/ok".parse().expect("uri")).expect("GET");
        let body = read_all_body(&mut response);
        let elapsed = start.elapsed();

        assert_eq!(
            body.len(),
            *size,
            "Response body length mismatch at {}: expected {}, got {}",
            size,
            body.len(),
            body.len()
        );

        // Verify integrity: repeating "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789" pattern
        let pattern = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
        for (i, &byte) in body.iter().enumerate() {
            assert_eq!(
                byte,
                pattern[i % pattern.len()],
                "Integrity mismatch at offset {} in {}-byte response",
                i,
                size
            );
        }

        let mbps = (*size as f64) / elapsed.as_secs_f64() / 1_048_576.0;

        eprintln!("  {} bytes: OK, {:?}, {:.1} MB/s", size, elapsed, mbps);
    }
}

/// Measure response encoding throughput for large bodies.
#[test]
fn test_large_response_throughput() {
    let size = 1_000_000; // 1 MB

    eprintln!(
        "\n=== Large Response Throughput ({} MB) ===",
        size / 1_048_576
    );

    let fixture = LargeResponseFixture::new(22100, size);
    let addr = fixture.base_url();

    let iterations = 10;
    let start = Instant::now();
    let mut total_bytes = 0u64;

    for _ in 0..iterations {
        let mut client = may_minihttp::client::HttpClient::connect(&*addr).expect("connect");
        let mut response = client.get("/ok".parse().expect("uri")).expect("GET");
        let body = read_all_body(&mut response);
        total_bytes += body.len() as u64;
        assert_eq!(body.len(), size);
    }

    let total = start.elapsed();
    let throughput = (total_bytes as f64) / total.as_secs_f64() / 1_048_576.0;

    eprintln!(
        "  total_bytes={}, time={:?}, throughput={:.2} MB/s",
        total_bytes, total, throughput
    );

    assert!(throughput > 0.0, "No throughput measured");
}

/// Test response encoding correctness across boundary sizes.
#[test]
fn test_response_body_boundary_sizes() {
    // Test sizes that stress different buffer boundaries (4KB internal buffer)
    let sizes = [1, 100, 1_024, 4_096, 4_097, 8_192, 16_384, 32_768];

    eprintln!("\n=== Response Body Boundary Sizes ===");

    for size in &sizes {
        let fixture = LargeResponseFixture::new(22200, *size);
        let addr = fixture.base_url();

        let mut client = may_minihttp::client::HttpClient::connect(&*addr).expect("connect");
        let mut response = client.get("/ok".parse().expect("uri")).expect("GET");
        let body = read_all_body(&mut response);

        assert_eq!(body.len(), *size, "Size mismatch at {}", size);

        // Verify Content-Length header matches actual body
        let cl = response
            .headers()
            .get("content-length")
            .unwrap()
            .to_str()
            .unwrap()
            .parse::<usize>()
            .unwrap();
        assert_eq!(
            cl, *size,
            "Content-Length mismatch at {} (got {})",
            size, cl
        );

        eprintln!("  {} bytes: OK (CL={})", size, cl);
    }
}
