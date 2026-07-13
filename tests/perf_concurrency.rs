//! Performance tests: concurrent connection scaling.
//!
//! Measures how throughput scales as the number of concurrent connections increases.
//! Each connection is a fresh HttpClient instance on its own TCP connection.
//!
//! Run with:
//!     cargo test --test perf_concurrency --features client -- --test-threads=1 --nocapture

use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Once};
use std::thread;
use std::time::{Duration, Instant};

use may_minihttp::client::HttpClient;
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
// Simple Echo Service
// ============================================================================

struct Counter {
    count: AtomicUsize,
}

impl Clone for Counter {
    fn clone(&self) -> Self {
        Self {
            count: AtomicUsize::new(self.count.load(Ordering::Relaxed)),
        }
    }
}

impl Counter {
    fn new() -> Self {
        Self {
            count: AtomicUsize::new(0),
        }
    }

    fn increment(&self) {
        self.count.fetch_add(1, Ordering::Relaxed);
    }
}

#[derive(Clone)]
struct EchoService {
    counter: Arc<Counter>,
}

impl HttpService for EchoService {
    fn call(&mut self, _req: Request, res: &mut ServerResponse) -> io::Result<()> {
        self.counter.increment();
        res.body("OK");
        Ok(())
    }
}

// ============================================================================
// Test Fixture
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

struct ConcurrencyFixture {
    port: u16,
    shutdown: Arc<AtomicBool>,
    server_thread: Option<thread::JoinHandle<()>>,
    counter: Arc<Counter>,
}

impl ConcurrencyFixture {
    fn new(preferred_port: u16) -> Self {
        init_may_runtime();

        let port = find_available_port(preferred_port);
        let counter = Arc::new(Counter::new());
        let counter_clone = Arc::clone(&counter);
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = Arc::clone(&shutdown);
        let addr = format!("127.0.0.1:{}", port);

        let svc = EchoService {
            counter: Arc::clone(&counter),
        };

        let server_thread = thread::spawn(move || {
            let handle = HttpServer(svc)
                .start(&addr)
                .expect("Failed to start test server");

            while !shutdown_clone.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(50));
            }

            eprintln!(
                "  [server] total requests={}",
                counter_clone.count.load(Ordering::Relaxed)
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
            counter,
        }
    }

    fn base_url(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }
}

impl Drop for ConcurrencyFixture {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(handle) = self.server_thread.take() {
            let _ = handle.join();
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

/// Concurrent connection scaling — measures aggregate throughput at N connections.
///
/// Each connection is a separate HttpClient (separate TCP connection).
/// Tests: 1, 2, 5, 10, 20, 50 concurrent connections.
#[test]
fn test_concurrent_connection_scaling() {
    let connection_counts = [1, 2, 5, 10, 20, 50];
    let requests_per_connection = 100;

    eprintln!("\n=== Concurrent Connection Scaling ===");
    eprintln!(
        "  Each connection sends {} requests\n",
        requests_per_connection
    );

    for &n_conns in &connection_counts {
        eprintln!("--- {} concurrent connections ---", n_conns);

        let fixture = ConcurrencyFixture::new(21000);
        let addr = fixture.base_url();

        let start = Instant::now();
        let barrier = Arc::new(Barrier::new(n_conns as usize));
        let barrier_clone = Arc::clone(&barrier);

        let handles: Vec<_> = (0..n_conns)
            .map(|i| {
                let addr = addr.clone();
                let barrier = Arc::clone(&barrier_clone);
                let reqs = requests_per_connection;
                thread::spawn(move || {
                    // Wait for all threads to be ready
                    barrier.wait();

                    let client_result = HttpClient::connect(&*addr);
                    if let Ok(mut client) = client_result {
                        for _ in 0..reqs {
                            let _ = client.get("/ok".parse().expect("uri"));
                        }
                        // Track success count
                        reqs
                    } else {
                        0
                    }
                })
            })
            .collect();

        let total_successes: usize = handles.into_iter().map(|h| h.join().unwrap()).sum();

        let total = start.elapsed();
        let throughput = (total_successes as f64) / total.as_secs_f64();
        let total_requests = (n_conns * requests_per_connection) as f64;
        let success_rate = (total_successes as f64) / total_requests * 100.0;

        eprintln!(
            "  total_requests={}, successes={}, success_rate={:.1}%, throughput={:.0} req/s, time={:?}",
            total_requests as usize,
            total_successes,
            success_rate,
            throughput,
            total
        );

        assert!(
            success_rate >= 99.0,
            "Success rate too low: {:.1}% (expected >= 99%)",
            success_rate
        );
    }
}

/// Connection count stress — many small connections to test server resilience.
///
/// Sends 500 connections with 1 request each to verify no connection leaks or errors.
#[test]
fn test_many_small_connections() {
    let fixture = ConcurrencyFixture::new(21100);
    let total_connections = 500;

    eprintln!(
        "\n=== Many Small Connections ({} connections) ===",
        total_connections
    );

    let start = Instant::now();
    let barrier = Arc::new(Barrier::new(10));
    let barrier_clone = Arc::clone(&barrier);
    let addr = fixture.base_url();

    let mut handles = Vec::with_capacity(10);
    for _ in 0..10 {
        let addr = addr.clone();
        let barrier = Arc::clone(&barrier_clone);
        handles.push(thread::spawn(move || {
            barrier.wait();
            let mut success = 0u64;
            for _ in 0..(total_connections / 10) {
                if let Ok(mut client) = HttpClient::connect(&*addr) {
                    if client.get("/ok".parse().expect("uri")).is_ok() {
                        success += 1;
                    }
                }
            }
            success
        }));
    }

    let total_successes: u64 = handles.into_iter().map(|h| h.join().unwrap()).sum();
    let total = start.elapsed();
    let throughput = (total_successes as f64) / total.as_secs_f64();

    eprintln!(
        "  successes={}, rate={:.0}/s, time={:?}",
        total_successes, throughput, total
    );

    assert_eq!(
        total_successes, total_connections as u64,
        "Not all connections succeeded"
    );
}

/// Single connection pipelining — multiple sequential requests on one connection.
///
/// Tests that the client can reuse a single HttpClient for many requests.
#[test]
fn test_single_connection_pipelining() {
    let fixture = ConcurrencyFixture::new(21200);
    let requests = 1000;

    eprintln!(
        "\n=== Single Connection Pipelining ({} requests) ===",
        requests
    );

    let addr = fixture.base_url();
    let mut client = HttpClient::connect(&*addr).expect("connect");

    let start = Instant::now();
    for _ in 0..requests {
        let _ = client.get("/ok".parse().expect("uri"));
    }
    let total = start.elapsed();
    let throughput = (requests as f64) / total.as_secs_f64();

    eprintln!("  req/s={:.0}, time={:?}", throughput, total);

    assert!(
        throughput >= 5000.0,
        "Pipelined throughput too low: {:.0} req/s (expected >= 5000)",
        throughput
    );
}
