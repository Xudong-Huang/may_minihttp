//! P0: Chunked Transfer-Encoding E2E test.
//!
//! The client's POST path uses `ChunkWriter` when no explicit Content-Length is set,
//! meaning POST bodies are sent as chunked on the wire. The server reads body bytes
//! from the stream regardless of encoding (it just reads Content-Length bytes or 0).
//! This tests that chunked POST bodies round-trip correctly.
//!
//! Run with:
//!     cargo test --test perf_chunked_e2e --features client -- --test-threads=1 --nocapture

use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Once};
use std::thread;
use std::time::Duration;

use may_minihttp::client::HttpClient;
use may_minihttp::{HttpServer, HttpService, Request, Response as ServerResponse};

static INIT: Once = Once::new();

fn init_may_runtime() {
    INIT.call_once(|| {
        let _ = may::config().set_stack_size(0x8000);
    });
}

/// Echoes received body bytes back in the response.
#[derive(Clone)]
struct EchoService {
    state: Arc<EchoState>,
}

#[derive(Default)]
struct EchoState {
    request_count: AtomicU64,
    total_body_bytes: AtomicU64,
}

impl Clone for EchoState {
    fn clone(&self) -> Self {
        Self {
            request_count: AtomicU64::new(self.request_count.load(Ordering::Relaxed)),
            total_body_bytes: AtomicU64::new(self.total_body_bytes.load(Ordering::Relaxed)),
        }
    }
}

impl HttpService for EchoService {
    fn call(&mut self, req: Request, res: &mut ServerResponse) -> io::Result<()> {
        self.state.request_count.fetch_add(1, Ordering::Relaxed);

        let mut body = Vec::new();
        let _ = req.body().read_to_end(&mut body);

        if !body.is_empty() {
            self.state
                .total_body_bytes
                .fetch_add(body.len() as u64, Ordering::Relaxed);
            res.body_mut().extend_from_slice(&body);
        } else {
            res.body("ok");
        }
        Ok(())
    }
}

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

struct ChunkedFixture {
    port: u16,
    shutdown: Arc<AtomicBool>,
    server_thread: Option<thread::JoinHandle<()>>,
    state: Arc<EchoState>,
}

impl ChunkedFixture {
    fn new(preferred_port: u16) -> Self {
        init_may_runtime();

        let port = find_available_port(preferred_port);
        let state = Arc::new(EchoState::default());
        let state_clone = Arc::clone(&state);
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = Arc::clone(&shutdown);
        let addr = format!("127.0.0.1:{port}");

        let svc = EchoService {
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
                state_clone.total_body_bytes.load(Ordering::Relaxed),
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

impl Drop for ChunkedFixture {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(handle) = self.server_thread.take() {
            let _ = handle.join();
        }
    }
}

fn read_body(client: &mut HttpClient, uri: &str, body: &[u8]) -> Vec<u8> {
    let mut response = client.post(uri.parse().expect("uri"), body).expect("POST");
    let mut buf = Vec::new();
    let _ = response.read_to_end(&mut buf);
    buf
}

/// POST round-trip correctness at various sizes.
#[test]
fn test_post_roundtrip() {
    let sizes = [1, 100, 1_000, 10_000];

    eprintln!("\n=== POST Round-Trip Correctness ===");

    for size in &sizes {
        let fixture = ChunkedFixture::new(23000);
        let addr = fixture.base_url();
        let body = vec![b'X'; *size];

        let mut client = HttpClient::connect(&*addr).expect("connect");
        let resp_body = read_body(&mut client, "/echo", &body);

        assert_eq!(
            resp_body.len(),
            *size,
            "Size mismatch at {}: sent {}, got {}",
            size,
            body.len(),
            resp_body.len(),
        );
        assert_eq!(resp_body, body);

        eprintln!("  {size} bytes: OK");
    }
}

/// POST body throughput measurement.
#[test]
fn test_post_throughput() {
    let fixture = ChunkedFixture::new(23100);
    let body = b"hello world chunked test";
    let iterations = 200;

    eprintln!("\n=== POST Body Throughput ===");

    let start = std::time::Instant::now();
    for _ in 0..iterations {
        let mut client = HttpClient::connect(&*fixture.base_url()).expect("connect");
        let _ = read_body(&mut client, "/echo", body);
    }
    let total = start.elapsed();

    let throughput = (iterations as f64) / total.as_secs_f64();
    let body_bytes = body.len() as f64 * iterations as f64;
    let mbps = (body_bytes / 1_048_576.0) / total.as_secs_f64();

    eprintln!(
        "  iterations={}, throughput={:.0} req/s, {:.2} MB/s",
        iterations, throughput, mbps,
    );

    assert!(throughput > 0.0, "Throughput not measured");
}

/// Verify server counter reflects all received bodies.
#[test]
fn test_chunked_server_counters() {
    let fixture = ChunkedFixture::new(23200);
    let addr = fixture.base_url();

    // Send several chunked POSTs
    let body1 = vec![b'A'; 50];
    let body2 = vec![b'B'; 100];
    let body3 = vec![b'C'; 200];

    {
        let mut client = HttpClient::connect(&*addr).expect("connect");
        let _ = read_body(&mut client, "/echo", &body1);
    }
    {
        let mut client = HttpClient::connect(&*addr).expect("connect");
        let _ = read_body(&mut client, "/echo", &body2);
    }
    {
        let mut client = HttpClient::connect(&*addr).expect("connect");
        let _ = read_body(&mut client, "/echo", &body3);
    }

    let req_count = fixture.state.request_count.load(Ordering::Relaxed);
    let body_bytes = fixture.state.total_body_bytes.load(Ordering::Relaxed);

    // -1 because check_ready() sends a GET that counts as a request
    assert_eq!(
        req_count, 4,
        "Expected 3 POSTs + 1 check_ready GET, got {}",
        req_count
    );
    assert_eq!(
        body_bytes, 350,
        "Expected 350 body bytes, got {}",
        body_bytes
    );

    eprintln!(
        "  request_count={}, total_body_bytes={}",
        req_count, body_bytes
    );
}
