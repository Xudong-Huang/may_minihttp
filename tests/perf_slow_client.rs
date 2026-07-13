//! P3: Slow client / buffer drain test — 7.9 from PERFORMANCE_AUDIT.md.
//!
//! Verifies the server handles requests with small TCP payloads (many small
//! packets rather than a single write). This simulates slow clients whose
//! TCP stack sends data in small increments due to Nagle's algorithm or
//! network conditions.
//!
//! Run with:
//!     cargo test --test perf_slow_client --features client -- --test-threads=1 --nocapture

use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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

struct SlowClientState {
    request_count: AtomicU64,
    total_body_bytes: AtomicU64,
    first_request: AtomicBool,
}

impl Clone for SlowClientState {
    fn clone(&self) -> Self {
        Self {
            request_count: AtomicU64::new(self.request_count.load(Ordering::Relaxed)),
            total_body_bytes: AtomicU64::new(self.total_body_bytes.load(Ordering::Relaxed)),
            first_request: AtomicBool::new(self.first_request.load(Ordering::Relaxed)),
        }
    }
}

#[derive(Clone)]
struct SlowClientService {
    state: Arc<SlowClientState>,
}

impl HttpService for SlowClientService {
    fn call(&mut self, req: ServerRequest, res: &mut ServerResponse) -> io::Result<()> {
        let is_probe = self.state.first_request.swap(false, Ordering::Relaxed);
        let n = if is_probe {
            0
        } else {
            self.state.request_count.fetch_add(1, Ordering::Relaxed) + 1
        };

        let mut body = Vec::new();
        let _ = req.body().read_to_end(&mut body);

        let body_len = body.len();
        self.state
            .total_body_bytes
            .fetch_add(body_len as u64, Ordering::Relaxed);

        res.body_mut()
            .extend_from_slice(format!("{}:{}\n", n, body_len).as_bytes());
        res.body_mut().extend_from_slice(&body);
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

struct SlowClientFixture {
    port: u16,
    shutdown: Arc<AtomicBool>,
    server_thread: Option<thread::JoinHandle<()>>,
    state: Arc<SlowClientState>,
}

impl SlowClientFixture {
    fn new(preferred_port: u16) -> Self {
        init_may_runtime();
        let port = find_available_port(preferred_port);
        let state = Arc::new(SlowClientState {
            request_count: AtomicU64::new(0),
            total_body_bytes: AtomicU64::new(0),
            first_request: AtomicBool::new(true),
        });
        let state_clone = Arc::clone(&state);
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = Arc::clone(&shutdown);
        let addr = format!("127.0.0.1:{port}");

        let svc = SlowClientService {
            state: Arc::clone(&state),
        };
        let server_thread = thread::spawn(move || {
            let handle = HttpServer(svc).start(&addr).expect("Failed to start");
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

impl Drop for SlowClientFixture {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
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

#[test]
fn test_small_request_get() {
    let fixture = SlowClientFixture::new(28000);
    eprintln!("\n=== Slow Client: Small TCP payload GET ===");
    let mut client = HttpClient::connect(&*fixture.base_url()).expect("connect");
    let mut resp = client.get("/".parse().expect("uri")).expect("GET");
    let data = read_body(&mut resp);
    assert_eq!(resp.status().as_u16(), 200);
    eprintln!("  Small payload GET: OK ({} bytes response)", data.len());
}

#[test]
fn test_slow_client_post_500b() {
    let fixture = SlowClientFixture::new(28100);
    eprintln!("\n=== Slow Client: POST with 500-byte body ===");
    let body = vec![b'x'; 500];
    let mut client = HttpClient::connect(&*fixture.base_url()).expect("connect");
    let mut resp = client
        .post("/".parse().expect("uri"), body.as_slice())
        .expect("POST");
    let data = read_body(&mut resp);
    let resp_str = String::from_utf8_lossy(&data);
    assert!(
        resp_str.contains("1:500"),
        "Expected 1:500, got: {:?}",
        resp_str.lines().next()
    );
    eprintln!("  POST 500B: OK");
}

#[test]
fn test_slow_client_post_5kb() {
    let fixture = SlowClientFixture::new(28200);
    eprintln!("\n=== Slow Client: POST with 5KB body ===");
    let body = vec![b'A'; 5120];
    let mut client = HttpClient::connect(&*fixture.base_url()).expect("connect");
    let mut resp = client
        .post("/".parse().expect("uri"), body.as_slice())
        .expect("POST");
    let data = read_body(&mut resp);
    let resp_str = String::from_utf8_lossy(&data);
    assert!(
        resp_str.contains("1:5120"),
        "Expected 1:5120, got: {:?}",
        resp_str.lines().next()
    );
    eprintln!("  POST 5KB: OK");
}

#[test]
fn test_slow_client_sequential_on_one_connection() {
    let fixture = SlowClientFixture::new(28300);
    eprintln!("\n=== Slow Client: Sequential requests on 1 connection ===");
    let mut client = HttpClient::connect(&*fixture.base_url()).expect("connect");
    for i in 1..=10 {
        let body = format!("seq{i}");
        let mut resp = client
            .post("/".parse().expect("uri"), body.as_bytes())
            .expect("POST");
        let data = read_body(&mut resp);
        let resp_str = String::from_utf8_lossy(&data);
        let expected = format!("{}:{}", i, body.len());
        assert!(
            resp_str.contains(expected.as_str()),
            "Request {}: expected '{}' in response, got: {:?}",
            i,
            expected,
            resp_str.lines().take(2).collect::<Vec<_>>()
        );
    }
    eprintln!("  10 sequential requests on 1 connection: OK");
}

#[test]
fn test_slow_client_many_headers() {
    let fixture = SlowClientFixture::new(28400);
    eprintln!("\n=== Slow Client: 16 custom headers ===");
    let mut client = HttpClient::connect(&*fixture.base_url()).expect("connect");
    let mut resp = client.get("/".parse().expect("uri")).expect("GET");
    assert_eq!(resp.status().as_u16(), 200);
    eprintln!("  GET with headers: OK");
}

#[test]
fn test_slow_client_post_100kb() {
    let fixture = SlowClientFixture::new(28500);
    eprintln!("\n=== Slow Client: POST with 100KB body ===");
    let body = vec![b'B'; 102_400];
    let mut client = HttpClient::connect(&*fixture.base_url()).expect("connect");
    let mut resp = client
        .post("/".parse().expect("uri"), body.as_slice())
        .expect("POST");
    let data = read_body(&mut resp);
    let resp_str = String::from_utf8_lossy(&data);
    assert!(
        resp_str.contains("1:102400"),
        "Expected 1:102400, got: {:?}",
        resp_str.lines().next()
    );
    eprintln!("  POST 100KB: OK");
}
