//! Performance tests: body size throughput scaling.
//!
//! Measures server throughput across body sizes and response sizes.
//! Also measures client read throughput for large responses.
//!
//! Run with:
//!     cargo test --test perf_body_throughput --features client -- --test-threads=1 --nocapture

use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Once};
use std::thread;
use std::time::{Duration, Instant};

use may_minihttp::client::{HttpClient, Response};
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
// Shared State
// ============================================================================

struct ServiceState {
    echo_body: Option<Vec<u8>>,
    fixed_body_size: usize,
    request_count: Arc<AtomicU64>,
    total_bytes_written: Arc<AtomicU64>,
}

impl Clone for ServiceState {
    fn clone(&self) -> Self {
        Self {
            echo_body: self.echo_body.clone(),
            fixed_body_size: self.fixed_body_size,
            request_count: Arc::clone(&self.request_count),
            total_bytes_written: Arc::clone(&self.total_bytes_written),
        }
    }
}

impl Default for ServiceState {
    fn default() -> Self {
        Self {
            echo_body: None,
            fixed_body_size: 0,
            request_count: Arc::new(AtomicU64::new(0)),
            total_bytes_written: Arc::new(AtomicU64::new(0)),
        }
    }
}

impl ServiceState {
    fn set_fixed_body_size(&mut self, size: usize) {
        self.fixed_body_size = size;
    }
}

#[derive(Clone)]
struct EchoService {
    state: Arc<ServiceState>,
}

impl HttpService for EchoService {
    fn call(&mut self, req: Request, res: &mut ServerResponse) -> io::Result<()> {
        let svc = &*self.state;
        svc.request_count.fetch_add(1, Ordering::Relaxed);

        let mut req_body = String::new();
        let _ = req.body().read_to_string(&mut req_body);

        match &svc.echo_body {
            Some(body) => {
                res.body_mut().extend_from_slice(body);
                svc.total_bytes_written
                    .fetch_add(body.len() as u64, Ordering::Relaxed);
            }
            None => {
                if !req_body.is_empty() {
                    res.body_mut().extend_from_slice(req_body.as_bytes());
                    svc.total_bytes_written
                        .fetch_add(req_body.len() as u64, Ordering::Relaxed);
                } else if svc.fixed_body_size > 0 {
                    let body = vec![b'X'; svc.fixed_body_size];
                    res.body_mut().extend_from_slice(&body);
                    svc.total_bytes_written
                        .fetch_add(svc.fixed_body_size as u64, Ordering::Relaxed);
                } else {
                    res.body("OK");
                }
            }
        }

        Ok(())
    }
}

// ============================================================================
// Test Fixture — owns ServiceState, shares Arc with server thread
// ============================================================================

/// Find an available port starting from preferred.
fn find_available_port(preferred: u16) -> u16 {
    for port in preferred..(preferred + 1000) {
        if TcpListener::bind(format!("127.0.0.1:{}", port)).is_ok() {
            return port;
        }
    }
    panic!("No available port in range {}", preferred);
}

/// Check if a server port is ready by sending a probe request.
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

struct ThroughputFixture {
    port: u16,
    shutdown: Arc<AtomicBool>,
    server_thread: Option<thread::JoinHandle<()>>,
    state: ServiceState,
    #[allow(dead_code)]
    state_for_thread: Arc<ServiceState>,
}

impl ThroughputFixture {
    fn new(preferred_port: u16) -> Self {
        init_may_runtime();

        let port = find_available_port(preferred_port);
        let state = ServiceState::default();
        let state_for_thread = Arc::new(state.clone());
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = Arc::clone(&shutdown);
        let addr = format!("127.0.0.1:{}", port);

        let svc = EchoService {
            state: Arc::clone(&state_for_thread),
        };

        let state_clone = Arc::clone(&state_for_thread);

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

        let state_for_thread_for_self = Arc::clone(&state_for_thread);

        Self {
            port,
            shutdown,
            server_thread: Some(server_thread),
            state,
            state_for_thread: state_for_thread_for_self,
        }
    }

    fn base_url(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }
}

impl Drop for ThroughputFixture {
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

fn read_body(response: &mut Response) -> Vec<u8> {
    let mut buf = Vec::new();
    let _ = response.read_to_end(&mut buf);
    buf
}

fn run_single_get(fixture: &ThroughputFixture) -> Duration {
    let addr = fixture.base_url();
    let mut client = HttpClient::connect(&*addr).expect("connect");
    let start = Instant::now();
    let _ = client.get("/ok".parse().expect("uri"));
    start.elapsed()
}

fn run_single_post(fixture: &ThroughputFixture, body: &[u8]) -> (Duration, Vec<u8>) {
    let addr = fixture.base_url();
    let mut client = HttpClient::connect(&*addr).expect("connect");
    let start = Instant::now();
    let mut response = client
        .post("/ok".parse().expect("uri"), body)
        .expect("POST");
    let elapsed = start.elapsed();
    let resp_body = read_body(&mut response);
    (elapsed, resp_body)
}

// ============================================================================
// Tests: Simple GET (no body)
// ============================================================================

/// Simple GET latency — p50/p95/p99.
#[test]
fn test_simple_get_latency() {
    let fixture = ThroughputFixture::new(20000);
    let iterations = 100;

    eprintln!("\n=== Simple GET Latency ({} iterations) ===", iterations);

    // Warm up
    for _ in 0..5 {
        run_single_get(&fixture);
    }

    let mut latencies = Vec::with_capacity(iterations as usize);
    for _ in 0..iterations {
        latencies.push(run_single_get(&fixture));
    }

    latencies.sort();
    let p50 = latencies[latencies.len() * 50 / 100];
    let p95 = latencies[latencies.len() * 95 / 100];
    let p99 = latencies[latencies.len() * 99 / 100];
    let total: Duration = latencies.iter().sum();
    let avg = total / iterations;
    let throughput = (iterations as f64) / total.as_secs_f64();

    eprintln!("  p50: {:?}", p50);
    eprintln!("  p95: {:?}", p95);
    eprintln!("  p99: {:?}", p99);
    eprintln!("  avg: {:?}", avg);
    eprintln!("  throughput: {:.0} req/s", throughput);

    assert!(p50 < Duration::from_millis(5), "p50 too high: {:?}", p50);
    assert!(p99 < Duration::from_millis(50), "p99 too high: {:?}", p99);
}

/// Simple GET throughput — requests per second.
#[test]
fn test_simple_get_throughput() {
    let fixture = ThroughputFixture::new(20001);
    let iterations = 500;

    eprintln!(
        "\n=== Simple GET Throughput ({} iterations) ===",
        iterations
    );

    // Warm up
    for _ in 0..10 {
        run_single_get(&fixture);
    }

    let start = Instant::now();
    for _ in 0..iterations {
        let _ = run_single_get(&fixture);
    }
    let total = start.elapsed();
    let throughput = (iterations as f64) / total.as_secs_f64();

    eprintln!("  total: {:?}", total);
    eprintln!("  throughput: {:.0} req/s", throughput);

    assert!(
        throughput >= 1000.0,
        "Expected >= 1000 req/s, got {:.0}",
        throughput
    );
}

// ============================================================================
// Tests: POST body size scaling
// ============================================================================

/// POST throughput across body sizes.
#[test]
fn test_post_body_size_scaling() {
    let sizes = [1, 100, 1000, 10_000, 100_000];
    let iterations_per_size = 50;

    eprintln!("\n=== POST Body Size Scaling ===");

    for size in &sizes {
        eprintln!("\n  --- {} bytes ---", size);
        let body = vec![b'A'; *size];
        let fixture = ThroughputFixture::new(20100);

        // Warm up
        for _ in 0..5 {
            let _ = run_single_post(&fixture, &body);
        }

        let start = Instant::now();
        let mut total_written = 0u64;
        for _ in 0..iterations_per_size {
            let (_elapsed, resp) = run_single_post(&fixture, &body);
            total_written += resp.len() as u64;
        }
        let total = start.elapsed();

        if total.as_secs() == 0 {
            eprintln!("  SKIPPED (zero time)");
            continue;
        }

        let throughput = (iterations_per_size as f64) / total.as_secs_f64();
        let mbps = (total_written as f64) / total.as_secs_f64() / 1_048_576.0;

        eprintln!("  req/s: {:.0}", throughput);
        eprintln!("  MB/s: {:.2}", mbps);
    }
}

/// POST body round-trip correctness at various sizes.
#[test]
fn test_post_body_correctness() {
    let sizes = [1, 100, 1_000, 10_000];

    eprintln!("\n=== POST Body Round-Trip Correctness ===");

    for size in &sizes {
        let body = vec![b'X'; *size];
        let fixture = ThroughputFixture::new(20200);

        let (elapsed, resp) = run_single_post(&fixture, &body);
        assert_eq!(
            resp.len(),
            *size,
            "Size mismatch at {}: sent {}, got {} (elapsed: {:?})",
            size,
            body.len(),
            resp.len(),
            elapsed
        );
        assert_eq!(resp, body, "Content mismatch at {}", size);

        eprintln!("  {}: OK ({} bytes, {:?})", size, body.len(), elapsed);
    }
}

// ============================================================================
// Tests: Response size scaling
// ============================================================================

/// Server sends fixed-size responses; client reads them.
#[test]
fn test_response_size_scaling() {
    let sizes = [0, 100, 1000, 10_000, 100_000];
    let iterations = 50;

    eprintln!("\n=== Response Size Scaling ===");

    for size in &sizes {
        eprintln!("\n  --- response {} bytes ---", size);
        let mut fixture = ThroughputFixture::new(20300);
        fixture.state.set_fixed_body_size(*size);

        // Warm up
        let _ = run_single_get(&fixture);

        let start = Instant::now();
        let mut total_written = 0u64;
        for _ in 0..iterations {
            let _elapsed = run_single_get(&fixture);
            let addr = fixture.base_url();
            let mut client = HttpClient::connect(&*addr).expect("connect");
            let mut rsp = client.get("/ok".parse().expect("uri")).expect("GET");
            total_written += read_body(&mut rsp).len() as u64;
        }
        let total = start.elapsed();

        if total.as_secs() == 0 {
            eprintln!("  SKIPPED (zero time)");
            continue;
        }

        let throughput = (iterations as f64) / total.as_secs_f64();
        let mbps = (total_written as f64) / total.as_secs_f64() / 1_048_576.0;

        eprintln!("  req/s: {:.0}", throughput);
        eprintln!("  MB/s: {:.2}", mbps);
    }
}

// ============================================================================
// Tests: Connection setup overhead
// ============================================================================

/// Measure connection setup cost (TCP connect + first response).
#[test]
fn test_connection_setup_overhead() {
    let fixture = ThroughputFixture::new(20400);
    let iterations = 200;

    eprintln!(
        "\n=== Connection Setup Overhead ({} iterations) ===",
        iterations
    );

    let first = run_single_get(&fixture);
    eprintln!("  first connection: {:?}", first);

    let start = Instant::now();
    for _ in 0..(iterations - 1) {
        let _ = run_single_get(&fixture);
    }
    let total = start.elapsed();
    let avg = total / (iterations - 1) as u32;

    eprintln!("  avg subsequent: {:?}", avg);
}
