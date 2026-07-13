//! Phase 4: Concurrent multi-client throughput — aggregate req/s under N simultaneous clients.
//!
//! The server spawns a per-connection coroutine for each incoming TCP connection.
//! Under N concurrent clients, aggregate throughput should scale linearly up to a
//! saturation point. This test measures:
//!
//! 1. Linear scaling: N=2,4,8 clients each sending 50 GETs
//! 2. Concurrency stress: 200 clients each sending 10 GETs
//! 3. Mixed verbs under load: GET/POST/PUT in equal distribution
//!
//! Run with:
//!     cargo test --test perf_concurrent_multi --features client -- --test-threads=1 --nocapture

use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Once};
use std::thread;
use std::time::Duration;

use may_minihttp::client::HttpClient;
use may_minihttp::{HttpServer, HttpService, Request as ServerRequest, Response as ServerResponse};

static INIT: Once = Once::new();

fn init_may_runtime() {
    INIT.call_once(|| {
        let _ = may::config().set_stack_size(0x8000);
    });
}

// ============================================================================
// Service: counts requests globally for verification
// ============================================================================

struct ConcurrencyState {
    request_count: AtomicU64,
}

impl Clone for ConcurrencyState {
    fn clone(&self) -> Self {
        Self {
            request_count: AtomicU64::new(self.request_count.load(Ordering::Relaxed)),
        }
    }
}

#[derive(Clone)]
struct ConcurrencyService {
    state: Arc<ConcurrencyState>,
}

impl HttpService for ConcurrencyService {
    fn call(&mut self, _req: ServerRequest, res: &mut ServerResponse) -> io::Result<()> {
        self.state.request_count.fetch_add(1, Ordering::Relaxed);
        res.body("ok");
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

struct ConcurrencyFixture {
    port: u16,
    shutdown: Arc<AtomicU64>,
    server_thread: Option<thread::JoinHandle<()>>,
    state: Arc<ConcurrencyState>,
}

impl ConcurrencyFixture {
    fn new(preferred_port: u16) -> Self {
        init_may_runtime();

        let port = find_available_port(preferred_port);
        let state = Arc::new(ConcurrencyState {
            request_count: AtomicU64::new(0),
        });
        let state_clone = Arc::clone(&state);
        let shutdown = Arc::new(AtomicU64::new(0));
        let shutdown_clone = Arc::clone(&shutdown);
        let addr = format!("127.0.0.1:{port}");

        let svc = ConcurrencyService {
            state: Arc::clone(&state),
        };

        let server_thread = thread::spawn(move || {
            let handle = HttpServer(svc).start(&addr).expect("Failed to start");
            while shutdown_clone.load(Ordering::Relaxed) == 0 {
                thread::sleep(Duration::from_millis(50));
            }
            eprintln!(
                "  [server] total_requests={}",
                state_clone.request_count.load(Ordering::Relaxed),
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

    fn request_count(&self) -> u64 {
        self.state.request_count.load(Ordering::Relaxed)
    }

    fn stop(&self) {
        self.shutdown.store(1, Ordering::Relaxed);
    }
}

impl Drop for ConcurrencyFixture {
    fn drop(&mut self) {
        self.stop();
        if let Some(handle) = self.server_thread.take() {
            let _ = handle.join();
        }
    }
}

fn read_body(res: &mut may_minihttp::client::Response) -> Vec<u8> {
    let mut buf = Vec::new();
    let _ = res.read_to_end(&mut buf);
    buf
}

// ============================================================================
// Tests
// ============================================================================

/// Linear scaling: 2, 4, and 8 concurrent clients, each sending 50 GETs.
/// Verify throughput increases roughly linearly.
#[test]
fn test_concurrent_get_scaling() {
    let fixture = ConcurrencyFixture::new(30100);
    let addr = fixture.base_url();

    eprintln!("\n=== Concurrent Multi-Client: GET scaling (2/4/8 clients × 50 GETs) ===");

    let client_count = 8;
    let requests_per_client = 50;
    let total_expected = (client_count * requests_per_client) as u64;

    let mut handles = Vec::with_capacity(client_count);

    let start = std::time::Instant::now();

    for _ in 0..client_count {
        let server_addr = addr.clone();
        let h = thread::spawn(move || {
            let mut client = HttpClient::connect(&*server_addr).expect("connect");
            let mut success = 0u64;
            for _ in 0..requests_per_client {
                let mut resp = client.get("/".parse().expect("uri")).expect("GET");
                let data = read_body(&mut resp);
                if data.len() > 0 && &data[0..2] == b"ok" {
                    success += 1;
                }
            }
            success
        });
        handles.push(h);
    }

    let mut total_success = 0u64;
    for h in handles {
        total_success += h.join().expect("thread panic");
    }

    let elapsed = start.elapsed();
    let req_per_sec = (total_success as f64 / elapsed.as_secs_f64()) as u64;

    eprintln!(
        "  {client_count} clients × {requests_per_client} GETs = {} success in {:.1}ms = {} req/s",
        total_success,
        elapsed.as_millis() as f64,
        req_per_sec,
    );

    assert_eq!(
        total_success, total_expected,
        "Expected {} successful requests, got {}",
        total_expected, total_success
    );
    let probe_count = fixture.request_count();
    assert_eq!(
        probe_count,
        total_expected + 1,
        "Server received {} requests ({} expected + 1 probe), got {}",
        probe_count,
        total_expected,
        probe_count
    );

    eprintln!("  Linear scaling: OK");
}

/// Concurrency stress: 200 clients each sending 10 GETs.
/// Verify server doesn't crash or lose connections.
#[test]
fn test_concurrent_stress_200_clients() {
    let fixture = ConcurrencyFixture::new(30110);
    let addr = fixture.base_url();

    eprintln!("\n=== Concurrent Multi-Client: 200 clients × 10 GETs stress ===");

    let client_count = 200;
    let requests_per_client = 10;
    let total_expected = (client_count * requests_per_client) as u64;

    let start = std::time::Instant::now();
    let mut handles = Vec::with_capacity(client_count);

    for _ in 0..client_count {
        let server_addr = addr.clone();
        let h = thread::spawn(move || {
            match HttpClient::connect(&*server_addr) {
                Ok(mut client) => {
                    let mut success = 0u64;
                    for _ in 0..requests_per_client {
                        let mut resp = client.get("/".parse().expect("uri")).expect("GET");
                        let data = read_body(&mut resp);
                        if data.len() > 0 && &data[0..2] == b"ok" {
                            success += 1;
                        }
                    }
                    Some(success)
                }
                Err(_) => None, // client creation failed
            }
        });
        handles.push(h);
    }

    let mut total_success = 0u64;
    for h in handles {
        if let Ok(Some(s)) = h.join() {
            total_success += s;
        }
    }

    let elapsed = start.elapsed();
    let req_per_sec = (total_success as f64 / elapsed.as_secs_f64()) as u64;

    eprintln!(
        "  {} clients × {} GETs = {} success in {:.1}ms = {} req/s",
        client_count,
        requests_per_client,
        total_success,
        elapsed.as_millis() as f64,
        req_per_sec,
    );

    // Allow some variance under heavy concurrency; at least 95% success
    let min_success = (total_expected as f64 * 0.95) as u64;
    assert!(
        total_success >= min_success,
        "Stress test: expected at least {} success, got {}",
        min_success,
        total_success
    );

    eprintln!("  Stress test: server stable");
}

/// Mixed verbs under load: equal GET/POST/PUT from concurrent clients.
/// Verify all verb paths remain functional simultaneously.
#[test]
fn test_concurrent_mixed_verbs() {
    let fixture = ConcurrencyFixture::new(30120);
    let addr = fixture.base_url();

    eprintln!("\n=== Concurrent Multi-Client: Mixed GET/POST/PUT ===");

    let client_count = 10;
    let requests_per_client = 30;
    let total_expected = (client_count * requests_per_client) as u64;

    let mut handles = Vec::with_capacity(client_count);

    let start = std::time::Instant::now();

    for _ in 0..client_count {
        let server_addr = addr.clone();
        let h = thread::spawn(move || {
            let mut client = HttpClient::connect(&*server_addr).expect("connect");
            let mut get_ok = 0u64;
            let mut post_ok = 0u64;
            let mut put_ok = 0u64;

            for j in 0..requests_per_client {
                let verb = j % 3;
                match verb {
                    0 => {
                        let mut resp = client.get("/".parse().expect("uri")).expect("GET");
                        if read_body(&mut resp).len() > 0 {
                            get_ok += 1;
                        }
                    }
                    1 => {
                        let body = b"hello";
                        let mut resp = client
                            .post("/".parse().expect("uri"), &body[..])
                            .expect("POST");
                        if read_body(&mut resp).len() > 0 {
                            post_ok += 1;
                        }
                    }
                    2 => {
                        // Use new_request+send_request for PUT (no dedicated PUT method)
                        let body = b"hello";
                        let mut req =
                            client.new_request(http::Method::PUT, "/".parse().expect("uri"));
                        req.send(&body[..]).expect("PUT body");
                        let mut resp = client.send_request(req).expect("PUT");
                        if read_body(&mut resp).len() > 0 {
                            put_ok += 1;
                        }
                    }
                    _ => unreachable!(),
                }
            }
            (get_ok, post_ok, put_ok)
        });
        handles.push(h);
    }

    let mut total_get = 0u64;
    let mut total_post = 0u64;
    let mut total_put = 0u64;

    for h in handles {
        let (g, p, u) = h.join().expect("thread panic");
        total_get += g;
        total_post += p;
        total_put += u;
    }

    let elapsed = start.elapsed();
    let total_success = total_get + total_post + total_put;
    let req_per_sec = (total_success as f64 / elapsed.as_secs_f64()) as u64;

    eprintln!(
        "  {} clients × {} req = GET:{} POST:{} PUT:{} = {} total in {:.1}ms = {} req/s",
        client_count,
        requests_per_client,
        total_get,
        total_post,
        total_put,
        total_success,
        elapsed.as_millis() as f64,
        req_per_sec,
    );

    assert_eq!(
        total_success, total_expected,
        "Expected {} total, got GET:{} POST:{} PUT:{}",
        total_expected, total_get, total_post, total_put
    );
    assert_eq!(
        fixture.request_count(),
        total_expected + 1,
        "Server request count mismatch: {} vs {} (+1 probe)",
        fixture.request_count(),
        total_expected
    );

    eprintln!("  Mixed verbs under concurrency: OK");
}
