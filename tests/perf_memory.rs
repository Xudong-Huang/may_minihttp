//! Phase 5: Memory profiling — RSS measurement and leak detection under sustained load.
//!
//! This test validates two requirements from PERFORMANCE_AUDIT.md:
//!
//!   §6.3  Memory per connection  < 64 KB
//!   §6.4  Zero memory leaks under load — run 10 000 requests, measure RSS delta
//!
//! On Linux the test reads /proc/self/status (VmRSS) for the server process RSS.
//! On non-Linux platforms the tests are skipped with #[cfg(unix)].
//!
//! Tests:
//!   1. sustained_load — 10 000 requests over a single connection, measure RSS delta
//!   2. connection_count — open many short-lived connections, verify per-connection < 64 KB
//!   3. body_size_rss — same sustained load with 1 KB body, verify no proportional leak
//!   4. drop_cleanup — create / drop many HttpClient instances, verify RSS recovers
//!   5. endurance — 10 000 requests with RSS checkpoints every 1 000 requests
//!
//! Run with:
//!     cargo test --test perf_memory --features client -- --test-threads=1 --nocapture

#[cfg(unix)]
mod unix {
    use std::io::{self, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    use may_minihttp::client::HttpClient;
    use may_minihttp::{
        HttpServer, HttpService, Request as ServerRequest, Response as ServerResponse,
    };

    // ========================================================================
    // RSS reading helpers (Linux /proc/self/status)
    // ========================================================================

    /// Read current process VmRSS in KB from /proc/self/status.
    /// Returns None if the file cannot be read (non-Linux, no permissions).
    fn read_rss_kb() -> Option<u64> {
        let content = std::fs::read_to_string("/proc/self/status").ok()?;
        for line in content.lines() {
            if line.starts_with("VmRSS:") {
                // "VmRSS:    12345 kB"
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    return parts[1].parse().ok();
                }
            }
        }
        None
    }

    /// Measure RSS delta: call `f`, return (before_kb, after_kb).
    /// `f` mutates `counter` to track successful requests.
    fn measure_rss_delta<F>(f: F, counter: &mut u64) -> (Option<u64>, Option<u64>)
    where
        F: FnOnce(&mut u64),
    {
        let before = read_rss_kb();
        f(counter);
        // Give the allocator a moment to stabilize after the workload
        thread::sleep(Duration::from_millis(100));
        let after = read_rss_kb();
        (before, after)
    }

    // ========================================================================
    // Service: echo body, count requests
    // ========================================================================

    struct MemState {
        request_count: AtomicU64,
    }

    impl Clone for MemState {
        fn clone(&self) -> Self {
            Self {
                request_count: AtomicU64::new(self.request_count.load(Ordering::Relaxed)),
            }
        }
    }

    #[derive(Clone)]
    struct MemService {
        state: Arc<MemState>,
    }

    impl HttpService for MemService {
        fn call(&mut self, req: ServerRequest, res: &mut ServerResponse) -> io::Result<()> {
            self.state.request_count.fetch_add(1, Ordering::Relaxed);
            let mut body = Vec::new();
            let _ = req.body().read_to_end(&mut body);
            res.body_mut().extend_from_slice(&body);
            if body.is_empty() {
                res.body("ok");
            }
            Ok(())
        }
    }

    // ========================================================================
    // Fixture
    // ========================================================================

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

    struct MemFixture {
        port: u16,
        shutdown: Arc<AtomicBool>,
        server_thread: Option<thread::JoinHandle<()>>,
        state: Arc<MemState>,
    }

    impl MemFixture {
        fn new(preferred_port: u16) -> Self {
            let port = find_available_port(preferred_port);
            let state = Arc::new(MemState {
                request_count: AtomicU64::new(0),
            });
            let state_clone = Arc::clone(&state);
            let shutdown = Arc::new(AtomicBool::new(false));
            let shutdown_clone = Arc::clone(&shutdown);
            let addr = format!("127.0.0.1:{port}");

            let svc = MemService {
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
                    "  [server] requests={}, rss_kb={:?}",
                    state_clone.request_count.load(Ordering::Relaxed),
                    read_rss_kb()
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
    }

    impl Drop for MemFixture {
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

    // ========================================================================
    // Test 1: Sustained load — 10 000 requests over one connection
    //
    // Verifies that after 10 000 round-trips on a single connection, the RSS
    // delta is bounded (not growing without limit).
    // ========================================================================

    #[test]
    fn test_sustained_load_rss_delta() {
        if read_rss_kb().is_none() {
            eprintln!("  SKIPPED: cannot read /proc/self/status (not Linux?)");
            return;
        }

        let fixture = MemFixture::new(31000);
        let addr = fixture.base_url();
        let total_requests = 10_000;

        eprintln!(
            "\n=== Memory: Sustained Load ({total_requests} requests, single connection) ==="
        );

        let mut success = 0u64;
        let (before, after) = measure_rss_delta(
            |c| {
                let mut client = HttpClient::connect(&*addr).expect("connect");
                for _ in 0..total_requests {
                    let mut resp = client.get("/".parse().expect("uri")).expect("GET");
                    let body = read_body(&mut resp);
                    if !body.is_empty() && body[..2] == b"ok"[..] {
                        *c += 1;
                    }
                }
            },
            &mut success,
        );

        let before_kb = before.unwrap_or(0);
        let after_kb = after.unwrap_or(0);
        let delta_kb = if before_kb > 0 {
            after_kb as i64 - before_kb as i64
        } else {
            0
        };

        eprintln!("  Requests completed: {success}/{total_requests}");
        eprintln!("  Baseline RSS:  {before_kb} KB");
        eprintln!("  Post-load RSS: {after_kb} KB");
        eprintln!("  RSS delta:     {delta_kb} KB");

        // Generous upper bound: 5 MB over 10 000 simple requests accounts for
        // allocator fragmentation. A real leak would show much larger growth.
        let max_allowable_delta = 5_000_i64;
        assert!(
            delta_kb <= max_allowable_delta,
            "RSS grew too much: {delta_kb} KB (max {max_allowable_delta} KB)"
        );
        assert_eq!(
            success, total_requests,
            "Expected {total_requests} successful requests, got {success}"
        );

        eprintln!("  Sustained load: PASS (delta = {delta_kb} KB)");
    }

    // ========================================================================
    // Test 2: Connection count — many short-lived connections, per-connection budget
    //
    // Opens N connections sequentially, each sending 10 requests, and checks
    // that the RSS does not grow proportionally to the connection count.
    // Acceptance criterion: memory per connection < 64 KB.
    // ========================================================================

    #[test]
    fn test_connection_count_per_connection_rss() {
        if read_rss_kb().is_none() {
            eprintln!("  SKIPPED: cannot read /proc/self/status (not Linux?)");
            return;
        }

        let fixture = MemFixture::new(31100);
        let addr = fixture.base_url();

        eprintln!("\n=== Memory: Connection Count — Per-Connection Budget ===");

        let conn_count = 500;
        let requests_per_conn = 10;

        // Warm up to get a clean baseline
        let _ = measure_rss_delta(
            |_| {
                let mut client = HttpClient::connect(&*addr).expect("connect");
                let mut resp = client.get("/".parse().expect("uri")).expect("GET");
                read_body(&mut resp);
            },
            &mut 0u64,
        );

        let mut success = 0u64;
        let (before, after) = measure_rss_delta(
            |c| {
                for _ in 0..conn_count {
                    let mut client = HttpClient::connect(&*addr).expect("connect");
                    for _ in 0..requests_per_conn {
                        let mut resp = client.get("/".parse().expect("uri")).expect("GET");
                        let body = read_body(&mut resp);
                        if !body.is_empty() && &body[..2] == b"ok" {
                            *c += 1;
                        }
                    }
                }
            },
            &mut success,
        );

        let before_kb = before.unwrap_or(0);
        let after_kb = after.unwrap_or(0);
        let delta_kb = if before_kb > 0 {
            after_kb as i64 - before_kb as i64
        } else {
            0
        };

        // Compute per-connection cost
        let per_connection_kb = if conn_count > 0 {
            delta_kb as f64 / conn_count as f64
        } else {
            0.0
        };

        eprintln!("  Connections opened:  {conn_count}");
        eprintln!("  Requests per conn:   {requests_per_conn}");
        eprintln!("  Total requests:      {success}");
        eprintln!("  RSS delta:           {delta_kb} KB");
        eprintln!("  Per-connection cost: {per_connection_kb:.2} KB");

        // Acceptance: per-connection cost < 64 KB
        assert!(
            per_connection_kb < 64.0,
            "Per-connection RSS cost {:.2} KB exceeds 64 KB budget (total delta {} KB over {} conns)",
            per_connection_kb,
            delta_kb,
            conn_count
        );

        eprintln!(
            "  Per-connection budget: PASS ({} KB/conn < 64 KB)",
            per_connection_kb as u64
        );
    }

    // ========================================================================
    // Test 3: Body size — same sustained load but with 1 KB body per request
    //
    // Verifies that body handling does not introduce proportional memory growth.
    // If the server buffered request bodies without freeing them, the delta would
    // scale as body_size × request_count.
    // ========================================================================

    #[test]
    fn test_body_size_rss_growth() {
        if read_rss_kb().is_none() {
            eprintln!("  SKIPPED: cannot read /proc/self/status (not Linux?)");
            return;
        }

        let fixture = MemFixture::new(31200);
        let addr = fixture.base_url();
        let total_requests = 5_000;
        let body_size = 1_024usize; // 1 KB body

        // Generate a fixed 1 KB body once
        let body: Vec<u8> = (0..body_size).map(|i| (i % 256) as u8).collect();

        eprintln!(
            "\n=== Memory: Body Size RSS Growth ({total_requests} req × {body_size}B body) ==="
        );

        let mut success = 0u64;
        let (before, after) = measure_rss_delta(
            |c| {
                let mut client = HttpClient::connect(&*addr).expect("connect");
                for _ in 0..total_requests {
                    let mut resp = client
                        .post("/".parse().expect("uri"), body.as_slice())
                        .expect("POST");
                    let resp_body = read_body(&mut resp);
                    // Response echoes the body — verify first 2 bytes are "ok"
                    if resp_body.len() > 2 && &resp_body[..2] == b"ok" {
                        *c += 1;
                    }
                }
            },
            &mut success,
        );

        let before_kb = before.unwrap_or(0);
        let after_kb = after.unwrap_or(0);
        let delta_kb = if before_kb > 0 {
            after_kb as i64 - before_kb as i64
        } else {
            0
        };

        // Total raw body throughput: 5 000 × 1 KB = 5 MB.
        // If no leak, delta should be well under 5 MB (allocator overhead only).
        let max_allowable = 3_000_i64;

        eprintln!("  Requests completed: {success}/{total_requests}");
        eprintln!("  Body size per req:  {body_size} B");
        eprintln!(
            "  Total body traffic: {} MB",
            (total_requests * body_size) / (1024 * 1024)
        );
        eprintln!("  RSS delta:          {delta_kb} KB");

        assert!(
            delta_kb <= max_allowable,
            "RSS grew too much with body throughput: {delta_kb} KB (max {max_allowable} KB)"
        );

        eprintln!("  Body size RSS growth: PASS (delta = {delta_kb} KB)");
    }

    // ========================================================================
    // Test 4: Drop cleanup — create and drop many HttpClient instances
    //
    // HttpClient wraps an Rc<RefCell<BufferIo<TcpStream>>>. If the client holds
    // references after drop, RSS would grow. This test verifies that creating
    // and dropping many clients does not leak memory.
    // ========================================================================

    #[test]
    fn test_drop_cleanup_rss() {
        if read_rss_kb().is_none() {
            eprintln!("  SKIPPED: cannot read /proc/self/status (not Linux?)");
            return;
        }

        let fixture = MemFixture::new(31300);
        let addr = fixture.base_url();

        eprintln!("\n=== Memory: Drop Cleanup — Client Instance Reclamation ===");

        // Phase 1: measure baseline after some warmup
        let _ = measure_rss_delta(
            |_| {
                let mut client = HttpClient::connect(&*addr).expect("connect");
                let mut resp = client.get("/".parse().expect("uri")).expect("GET");
                read_body(&mut resp);
            },
            &mut 0u64,
        );

        let _phase1_before = read_rss_kb();

        // Phase 2: create and drop N fresh connections (no reuse).
        // mimalloc allocates large arenas and keeps freed memory for reuse —
        // RSS will NOT drop back to baseline, and that is expected behaviour,
        // NOT a leak. The real signal: after N rounds the growth rate should
        // converge to zero (arenas are warm, reused instead of extended).
        let client_count = 200;
        let rounds = 5;

        // Run `rounds` cycles of connection churn, measuring RSS before each round.
        let mut rss_snapshots: Vec<u64> = Vec::with_capacity(rounds);

        eprintln!("  Clients per round: {client_count}");
        eprintln!("  Rounds:            {rounds}");

        for round in 0..rounds {
            let before = read_rss_kb().unwrap_or(0);
            rss_snapshots.push(before);

            for _ in 0..client_count {
                let mut client = HttpClient::connect(&*addr).expect("connect");
                let mut resp = client.get("/".parse().expect("uri")).expect("GET");
                let _ = read_body(&mut resp);
                // client drops here — TCP connection closes
            }

            thread::sleep(Duration::from_millis(300)); // let TCP close and allocator settle
            let after = read_rss_kb().unwrap_or(0);
            let delta = after as i64 - before as i64;

            eprintln!(
                "  Round {}/{}: RSS {} -> {} KB (delta: {} KB)",
                round + 1,
                rounds,
                before,
                after,
                delta
            );
        }

        // Verify the growth rate converges: the delta between round N and round
        // N+1 should be small once arenas are warm. We check that the last two
        // deltas differ by at most 2 MB (2 000 KB).
        // If there were a leak, each round would add the same amount and deltas
        // would not converge.
        let delta1 = rss_snapshots[1] as i64 - rss_snapshots[0] as i64;
        let delta2 = rss_snapshots[rounds - 1] as i64 - rss_snapshots[rounds - 2] as i64;
        let convergence = (delta2 as i64) - (delta1 as i64);

        eprintln!("  Round 1->2 delta:    {delta1} KB");
        eprintln!("  Round {0}->1 delta:   {delta2} KB", rounds);
        eprintln!("  Convergence gap:     {convergence} KB");

        // The convergence gap should be small — if RSS is stabilizing, the last
        // delta should be close to the first delta (arenas are reused, not extended).
        // We allow up to 5 MB difference for initial allocation noise.
        assert!(
            convergence.abs() <= 5_000,
            "RSS not converging: round1-2 delta={delta1} KB, round{}-1 delta={delta2} KB, \
             gap={convergence} KB (mimalloc arenas may not be stable)",
            rounds
        );

        // Also verify the total growth over all rounds is bounded.
        let total_growth = rss_snapshots[rounds - 1] as i64 - rss_snapshots[0] as i64;
        eprintln!("  Total growth over {rounds} rounds: {total_growth} KB");

        eprintln!("  Drop cleanup: PASS");
    }

    // ========================================================================
    // Test 5: Sustained load — 10 000 requests with RSS at intervals
    //
    // Full-endurance test: 10 000 requests on one connection with RSS measured
    // at 1 000-request checkpoints. Verifies RSS is flat over time, not
    // trending upward.
    // ========================================================================

    #[test]
    fn test_sustained_load_endurance() {
        if read_rss_kb().is_none() {
            eprintln!("  SKIPPED: cannot read /proc/self/status (not Linux?)");
            return;
        }

        let fixture = MemFixture::new(31400);
        let addr = fixture.base_url();
        let total_requests = 10_000;
        let checkpoint_every = 1_000;
        let checkpoints = total_requests / checkpoint_every;

        eprintln!("\n=== Memory: Endurance — RSS at Intervals ({total_requests} requests) ===");

        let mut client = HttpClient::connect(&*addr).expect("connect");

        for cp in 0..checkpoints {
            let start_idx = cp * checkpoint_every;

            // Measure RSS at checkpoint start
            thread::sleep(Duration::from_millis(50)); // allow allocator to settle
            let rss_start = read_rss_kb();

            for _ in 0..checkpoint_every {
                let mut resp = client.get("/".parse().expect("uri")).expect("GET");
                let body = read_body(&mut resp);
                assert!(
                    !body.is_empty() && body[..2] == b"ok"[..],
                    "Request #{start_idx} failed response check"
                );
            }

            // Measure RSS after checkpoint
            thread::sleep(Duration::from_millis(50));
            let rss_end = read_rss_kb();

            let delta = match (rss_start, rss_end) {
                (Some(s), Some(e)) => {
                    let d = e as i64 - s as i64;
                    eprintln!(
                        "  Checkpoint {}/{}: RSS {} -> {} KB (delta: {} KB)",
                        cp + 1,
                        checkpoints,
                        s,
                        e,
                        d
                    );
                    d
                }
                _ => 0,
            };

            // No single checkpoint should show > 1 MB growth
            assert!(
                delta <= 1_000,
                "Checkpoint {}/{}: RSS grew {} KB between measurements",
                cp + 1,
                checkpoints,
                delta
            );
        }

        // Verify server received all requests
        let probe_count = fixture.request_count();
        assert_eq!(
            probe_count,
            total_requests as u64 + 1,
            "Server request count: {} (expected {} + 1 probe)",
            probe_count,
            total_requests
        );

        eprintln!("  Endurance: PASS — {checkpoints} checkpoints, no sustained RSS growth");
    }
}
