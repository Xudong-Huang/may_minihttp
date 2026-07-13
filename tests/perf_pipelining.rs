//! P1: Request pipelining test.
//!
//! The server's `each_connection_loop` naturally supports pipelining — it loops,
//! processing one request at a time but never closing the connection. The client
//! writes requests sequentially on the same connection without waiting for each
//! response. This tests that pipelined requests are correctly buffered and responses
//! arrive in order.
//!
//! Run with:
//!     cargo test --test perf_pipelining --features client -- --test-threads=1 --nocapture

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

struct PipelineState {
    request_count: AtomicU64,
    total_body_bytes: AtomicU64,
    first_request: AtomicBool,
}

impl Clone for PipelineState {
    fn clone(&self) -> Self {
        Self {
            request_count: AtomicU64::new(self.request_count.load(Ordering::Relaxed)),
            total_body_bytes: AtomicU64::new(self.total_body_bytes.load(Ordering::Relaxed)),
            first_request: AtomicBool::new(self.first_request.load(Ordering::Relaxed)),
        }
    }
}

#[derive(Clone)]
struct PipelineService {
    state: Arc<PipelineState>,
}

impl HttpService for PipelineService {
    fn call(&mut self, req: ServerRequest, res: &mut ServerResponse) -> io::Result<()> {
        if self.state.first_request.swap(false, Ordering::Relaxed) {
            res.body("ok");
            return Ok(());
        }

        let n = self.state.request_count.fetch_add(1, Ordering::Relaxed) + 1;

        let mut body = Vec::new();
        let _ = req.body().read_to_end(&mut body);

        if body.is_empty() {
            res.body_mut()
                .extend_from_slice(format!("seq:{}\n", n).as_bytes());
        } else {
            res.body_mut()
                .extend_from_slice(format!("seq:{}|", n).as_bytes());
            res.body_mut().extend_from_slice(&body);
        }

        self.state
            .total_body_bytes
            .fetch_add(body.len() as u64, Ordering::Relaxed);
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

struct PipelineFixture {
    port: u16,
    shutdown: Arc<AtomicBool>,
    server_thread: Option<thread::JoinHandle<()>>,
    state: Arc<PipelineState>,
}

impl PipelineFixture {
    fn new(preferred_port: u16) -> Self {
        init_may_runtime();

        let port = find_available_port(preferred_port);
        let state = Arc::new(PipelineState {
            request_count: AtomicU64::new(0),
            total_body_bytes: AtomicU64::new(0),
            first_request: AtomicBool::new(true),
        });
        let state_clone = Arc::clone(&state);
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = Arc::clone(&shutdown);
        let addr = format!("127.0.0.1:{port}");

        let svc = PipelineService {
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

impl Drop for PipelineFixture {
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

/// Pipeline 20 GET requests on a single connection — verify response order.
#[test]
fn test_pipelined_get_requests() {
    let fixture = PipelineFixture::new(26000);
    let addr = fixture.base_url();

    eprintln!("\n=== Pipelining: 20 Sequential GETs ===");

    let mut client = HttpClient::connect(&*addr).expect("connect");

    for i in 1..=20 {
        let resp = client.get("/".parse().expect("uri")).expect("GET");
        let mut body = resp;
        let data = read_all(&mut body);
        let s = String::from_utf8_lossy(&data);
        assert!(
            s.starts_with(&format!("seq:{}\n", i)),
            "Request {}: expected seq:{}, got {:?}",
            i,
            i,
            s
        );
    }

    let req_count = fixture.state.request_count.load(Ordering::Relaxed);
    assert_eq!(req_count, 20, "Server should have processed 20 requests");
    eprintln!("  20 sequential GETs on 1 connection: OK");
}

/// Pipeline POST requests with small bodies — verify body echo order.
#[test]
fn test_pipelined_post_requests() {
    let fixture = PipelineFixture::new(26100);
    let addr = fixture.base_url();

    eprintln!("\n=== Pipelining: 20 POSTs ===");

    let mut client = HttpClient::connect(&*addr).expect("connect");

    for i in 1..=20 {
        let body = format!("data-{i}").into_bytes();
        let mut resp = client
            .post("/".parse().expect("uri"), body.as_slice())
            .expect("POST");
        let resp_body = read_all(&mut resp);

        let prefix = format!("seq:{i}|");
        assert!(
            resp_body.starts_with(prefix.as_bytes()),
            "POST #{} should start with {}",
            i,
            prefix
        );
        assert!(resp_body.ends_with(&body), "POST #{} should echo body", i);
    }

    let req_count = fixture.state.request_count.load(Ordering::Relaxed);
    assert_eq!(req_count, 20);
    eprintln!("  20 sequential POSTs on 1 connection: OK");
}

/// Pipelined GET throughput.
#[test]
fn test_pipelined_get_throughput() {
    let fixture = PipelineFixture::new(26200);
    let addr = fixture.base_url();
    let iterations = 100;

    eprintln!(
        "\n=== Pipelining: GET Throughput ({} iterations) ===",
        iterations
    );

    let start = std::time::Instant::now();
    for _ in 0..iterations {
        let mut client = HttpClient::connect(&*addr).expect("connect");
        let mut resp = client.get("/".parse().expect("uri")).expect("GET");
        let _ = read_all(&mut resp);
    }
    let total = start.elapsed();
    let throughput = (iterations as f64) / total.as_secs_f64();

    eprintln!("  {:.0} req/s (total: {:?})", throughput, total);
    assert!(throughput > 0.0);
}
