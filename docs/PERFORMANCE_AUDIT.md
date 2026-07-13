# may_minihttp Comprehensive Performance & Coverage Audit

## 1. SYSTEM OVERVIEW

may_minihttp is a coroutine-based HTTP/1.1 client and server built on the `may` runtime stack. The library provides both server-side HTTP handling and a native HTTP client (under the `client` feature flag).

### Architecture Summary
- **Server**: `may::net::TcpStream` → coroutine per connection → `httparse`-based request parser → `HttpService` trait → `Response` builder → wire encoding
- **Client**: `HttpClient` → `BufferIo<TcpStream>` → `http` crate integration → `BodyReader`/`BodyWriter` → TCP connection
- **Runtime**: `may` stackful coroutines with non-blocking I/O on Unix, blocking I/O elsewhere
- **Dependencies**: `httparse` (parsing), `bytes` (buffer management), `itoa` (number formatting), `may` (coroutines/networking)

---

## 2. SERVER-SIDE CAPABILITIES

### 2.1 HTTP/1.1 Protocol Support

| Feature | Implemented | Test Coverage |
|---------|-------------|---------------|
| Request line parsing (method, path, version) | Yes | Partial — request_parsing.rs |
| Header line parsing (httparse) | Yes | Partial — header_traffic_integration.rs |
| Body parsing (Content-Length based) | Yes | Limited |
| Chunked Transfer-Encoding | Parsed (no server-side encoding) | No E2E test |
| HTTP/1.0 requests | Yes | Partial |
| HTTP/1.1 requests | Yes | Partial |
| Host header parsing | Yes | client_integration.rs |
| Content-Length body reading | Yes | Partial — body_reader.rs unit tests |
| Connection: keep-alive / pipelining | Yes | **No E2E perf test** |
| Connection: close | Yes | **No E2E perf test** |
| Malformed request rejection | Yes | request_parsing.rs |
| Header line count enforcement | Yes | max_headers_enum.rs, header_traffic_integration.rs |

### 2.2 Response Building

| Feature | Implemented | Test Coverage |
|---------|-------------|---------------|
| Status code + message | Yes | response.rs unit tests |
| Static headers (zero allocation) | Yes | response.rs: `header_static_is_zero_alloc` |
| Owned headers (String/Box/Cow) | Yes | response.rs: `header_owned_variants_are_accepted` |
| Content-Type header support | Yes | Caller sets, no verification test |
| Content-Length header | Auto-encodes | response.rs unit tests |
| Server header | Auto ("M") | client_integration.rs |
| Date header | Auto | client_integration.rs |
| Body as `&str` | Yes | response.rs |
| Body as `Vec<u8>` | Yes | response.rs |
| Body as `&mut BytesMut` | Yes | response.rs |
| Error responses (500) | Yes | **No E2E test** |
| ResponseHeader::Static("Connection: keep-alive") | Caller sets | No E2E verification |

### 2.3 Server Architecture

| Feature | Implemented | Test Coverage |
|---------|-------------|---------------|
| HttpServer (16 headers, default) | Yes | simple_header_test.rs |
| HttpServerWithHeaders<T, N> (custom N) | Yes | max_headers_enum.rs |
| HttpService trait | Yes | Examples |
| HttpServiceFactory trait | Yes | examples/techempower.rs |
| Coroutines per connection | Yes | **No concurrency perf test** |
| Unix non-blocking I/O (epoll) | Yes | **No slow-client stress test** |
| Non-Unix blocking I/O | Yes | **No coverage** |
| Client disconnect handling | Yes | Internal logic, no E2E test |
| Buffer sizes (REQ/RSP: 32KB, body: 4KB) | Yes | **No buffer-boundary perf test** |
| Windows WSAECONNREFUSED remap | Yes | **No Windows test** |

---

## 3. CLIENT-SIDE CAPABILITIES (feature="client")

### 3.1 HTTP/1.1 Protocol

| Feature | Implemented | Test Coverage |
|---------|-------------|---------------|
| GET | Yes | client_integration.rs |
| POST | Yes | client_integration.rs |
| HEAD | Yes | client_integration.rs |
| PUT | Yes | client_integration.rs |
| DELETE | Yes | client_integration.rs |
| PATCH | Yes | client_integration.rs |
| OPTIONS | Yes | client_integration.rs |
| Request body (sized/Content-Length) | Yes | client/request.rs unit tests |
| Request body (chunked) | Yes | client/body/body_writer.rs unit tests |
| Response body (sized) | Yes | client/body/body_reader.rs unit tests |
| Response body (chunked decode) | Yes | client/body/body_reader.rs unit tests |
| Empty/no-body responses (HEAD) | Yes | client/response.rs unit tests |
| Keep-alive / connection reuse | **No** | N/A |
| Request pipelining | **No** | N/A |
| Host header injection | Yes | client_integration.rs |
| User-Agent header | Auto ("may_minihttp") | client_integration.rs |
| Accept header | Auto ("*/*") | client_integration.rs |
| Connection close after response | **No** | N/A |
| HTTP/1.0 vs HTTP/1.1 responses | Yes | client/response.rs unit tests |
| Malformed response detection | Yes | client/response.rs unit tests |

### 3.2 Client Architecture

| Feature | Implemented | Test Coverage |
|---------|-------------|---------------|
| HttpClient::connect() | Yes | client_integration.rs |
| HttpClient::set_timeout() | Yes | **No E2E timeout test** |
| BufferIo<TcpStream> | Yes | client/buffer.rs unit tests |
| Shared connection via Rc<RefCell> | Yes | **No shared-state stress test** |
| Request builder pattern (http crate) | Yes | client/request.rs unit tests |
| Response struct wrapping (http crate) | Yes | client/response.rs unit tests |

---

## 4. CURRENT TEST INVENTORY

### 4.1 Unit Tests (in-source, `--lib --all-features`)

**response.rs** (3 tests):
- `header_static_is_zero_alloc` — verifies Static fast path has no allocation
- `header_owned_variants_are_accepted` — verifies String/Box/Cow take Owned variant
- `encode_mixes_static_and_owned_headers` — verifies encode() writes both correctly

**client/request.rs** (3 tests):
- `delete_without_body_writes_head_on_drop` — verifies DELETE sends HEAD on drop
- `put_with_sized_body_writes_content_length` — verifies PUT with body sets Content-Length
- `patch_and_options_do_not_panic` — verifies PATCH/OPTIONS don't panic on drop

**client/response.rs** (7 tests):
- `test_decode_valid_200`, `test_decode_partial`, `test_decode_content_length`
- `test_decode_http10`, `test_decode_malformed`
- `test_decode_set_reader_with_expect_body`, `test_decode_set_reader_no_body`
- `test_decode_set_reader_bad_cl`

**client/body/body_reader.rs** (10 tests):
- `test_eat_valid`, `test_eat_invalid`
- `test_read_chunk_size_basic/small/with_extension/zero/invalid`
- `test_sized_reader_exact_bytes`, `test_sized_reader_zero_remain`
- `test_chunk_reader_multiple_chunks/chunk_extensions/early_eof`
- `test_empty_reader_always_zero`, `test_drop_consumes_remaining_chunks`

**client/body/body_writer.rs** (7 tests):
- `test_sized_writer_exact_bytes`, `test_sized_writer_over_limit`
- `test_sized_writer_drop_fills_padding`
- `test_chunk_writer_format/multiple_writes/drop_terminator`
- `test_empty_writer_accepts_no_data`

**client/buffer.rs** (3 tests):
- `test_consume_and_get_buf`, `test_resize`, `test_write`

**Total unit tests: 33**

### 4.2 Integration Tests

**simple_header_test.rs** (4 tests): Header limits (3, 16, 17, 20, 32 headers)
**request_parsing.rs** (12 tests): Various HTTP request parsing scenarios
**max_headers_enum.rs** (19 tests): MaxHeaders enum behavior, size selections, edge cases
**header_traffic_integration.rs** (15 tests): Large headers, user-agent, cookies, referer, authorization, api-gateway, buffered requests
**client_integration.rs** (20 tests): All HTTP verbs, chunked, keep-alive, timeout, connection reuse, different URIs, malformed

**Total integration tests: ~70**

### 4.3 Load Tests

**goose_header_load_test.rs** (6 tests):
- Smoke test with single user/second
- Varying headers count
- Browser-like traffic simulation
- Load balancer traffic simulation
- High header stress test (30+ headers)
- Large header value test (2KB+ values)

---

## 5. GAP ANALYSIS

### 5.1 Critical Gaps (High Severity)

| Gap | Domain | Current State | Impact |
|-----|--------|--------------|--------|
| **Body size throughput scaling** | Server + Client | No test measures throughput across body sizes | Cannot determine optimal buffer sizing; unknown performance at 1KB-1MB bodies |
| **Concurrent connection scaling** | Server | No test measures throughput vs connection count (1-100) | Unknown scalability limits; coroutine scheduler performance unmeasured |
| **Pipelined request throughput** | Server | Supports multi-request-per-connection loop | No E2E test measures how many requests pipelined before buffer fills |
| **Keep-alive connection overhead** | Server + Client | Server supports it; client does NOT | Connection setup cost is unknown baseline |
| **Large body POST throughput** | Client + Server | BodyWriter has unit tests | No E2E test for 10KB-1MB POST body transfer end-to-end |
| **Response body read throughput** | Client | BodyReader has unit tests | No E2E test measuring client read performance at 1KB-10MB |
| **Concurrent multi-client throughput** | Server | Server spawns per-connection coroutines | No test measures aggregate throughput under N concurrent clients |

### 5.2 Medium Gaps (Moderate Severity)

| Gap | Domain | Current State | Impact |
|-----|--------|--------------|--------|
| **Chunked body E2E round-trip** | Client + Server | BodyWriter has unit tests, no E2E | Cannot verify chunked encoding works correctly end-to-end |
| **Multi-header E2E (32/64/128)** | Server | Unit tests for MaxHeaders enum, no wire test | Cannot verify server accepts/rejects headers at different limits under load |
| **All HTTP verbs via native HttpClient** | Client | All verbs tested via may_http client, NOT native client | Native HttpClient verb support untested |
| **Timeout behavior** | Client | `set_timeout()` exists | No test verifies timeout actually triggers correctly |
| **Slow client / buffer drain** | Server | Nonblock I/O implemented | No test sends data byte-by-byte to verify buffer logic |
| **Large response body** | Client | BodyReader has unit tests | No E2E test for 1MB+ server response |
| **Custom headers round-trip** | Client + Server | Header parsing tested, no E2E | Unknown if custom headers are correctly encoded/decoded |

### 5.3 Low Gaps (Minor Severity)

| Gap | Domain | Current State | Impact |
|-----|--------|--------------|--------|
| **Error response wire format** | Server | `encode_error()` exists | No E2E test sends request that triggers 500 response |
| **Malformed response E2E** | Client | Unit tests decode error | No E2E test sends garbage response to verify client error handling |
| **HTTP/1.0 vs HTTP/1.1 wire correctness** | Server | Response differs by version | No wire-level test verifies both paths |
| **Windows platform test** | Client | WSAECONNREFUSED remap implemented | No Windows CI test (no Windows runner) |
| **Large number of headers (256)** | Server | Custom(256) is max | No E2E test at limit |

---

## 6. NON-FUNCTIONAL REQUIREMENTS (PERFORMANCE TARGETS)

### 6.1 Throughput Requirements

| Metric | Target | Measurement Method |
|--------|--------|-------------------|
| Single connection, simple response (body < 100B) | >= 5,000 req/s | Goose with 10 concurrent users, 60s duration |
| Single connection, medium response (body 1KB-10KB) | >= 2,000 req/s | Goose with 5 concurrent users, 60s duration |
| POST with body (1KB-10KB) | >= 1,000 req/s | Goose with 5 concurrent users, 60s duration |
| Large response (body 100KB-1MB) | >= 50 MB/s throughput | Custom benchmark measuring MB/s |
| Large POST body (100KB-1MB) | >= 50 MB/s throughput | Custom benchmark measuring MB/s |

### 6.2 Latency Requirements

| Metric | Target | Measurement Method |
|--------|--------|-------------------|
| P50 latency (simple response) | < 2ms | Single connection, sequential requests |
| P95 latency (simple response) | < 10ms | Single connection, 1000 requests |
| P99 latency (simple response) | < 50ms | Single connection, 1000 requests |
| P50 latency (1KB response) | < 5ms | Single connection, sequential requests |
| P95 latency (1KB response) | < 20ms | Single connection, 1000 requests |
| Connection setup cost | < 1ms | Measure TCP connect + first response |

### 6.3 Scalability Requirements

| Metric | Target | Measurement Method |
|--------|--------|-------------------|
| 10 concurrent connections | >= 50,000 req/s aggregate | 10 goose users, simple response |
| 20 concurrent connections | >= 80,000 req/s aggregate | 20 goose users, simple response |
| 50 concurrent connections | >= 100,000 req/s aggregate | 50 goose users, simple response |
| Memory per connection | < 64KB | Track RSS increase per connection |
| Max sustained connections | >= 100 | Test until error/timeout |

### 6.4 Reliability Requirements

| Metric | Target | Verification |
|--------|--------|-------------|
| Zero panics under load | 100% | Run 100,000 requests, count panics |
| Zero memory leaks under load | 100% | Run 10,000 requests, measure RSS delta |
| Correct header handling at limits | 100% | Send 16/32/64/128 headers, verify correct behavior |
| Malformed request rejection | 100% | Send malformed requests, verify 4xx/5xx |
| Connection drop handling | 100% | Kill connections mid-request, verify no panic/crash |

---

## 7. FUNCTIONAL REQUIREMENTS FOR TEST SUITE

### 7.1 Body Size Throughput Test

**Requirement:** Measure server throughput across body sizes.
**Method:**
- Server echoes request body back in response
- POST with body sizes: 1B, 100B, 1KB, 10KB, 100KB, 1MB
- Run with 1, 5, 10 concurrent goose users for 60s each
- Measure: req/s, MB/s, p50/p95/p99 latency

**Acceptance Criteria:**
- [ ] Throughput scales linearly with body size up to buffer capacity
- [ ] No request drops or errors at any body size
- [ ] Latency increases predictably with body size (not exponentially)
- [ ] At 1MB body, throughput >= 50 MB/s on single connection

### 7.2 Concurrent Connection Scaling Test

**Requirement:** Measure server throughput as concurrent connections increase.
**Method:**
- Simple GET endpoint returning 50-byte response
- Run goose with 1, 5, 10, 20, 50, 100 concurrent users
- Each user sends 1000 requests, 60s duration
- Measure: aggregate req/s, per-connection req/s, latency distribution

**Acceptance Criteria:**
- [ ] Throughput scales linearly up to 20 connections
- [ ] Throughput scales sub-linearly but positively up to 100 connections
- [ ] No connection errors (BrokenPipe, timeout) exceed 0.1%
- [ ] No panics or crashes at any connection count

### 7.3 Pipeline Request Test

**Requirement:** Measure how many requests a single connection can pipeline.
**Method:**
- Client sends N requests sequentially without waiting for response
- Server processes and responds to each in order
- Measure: requests in flight, throughput gain vs sequential

**Acceptance Criteria:**
- [ ] Server correctly handles up to 10 pipelined requests per connection
- [ ] Responses return in correct request order
- [ ] No response interleaving or corruption

### 7.4 Chunked Body E2E Test

**Requirement:** Verify chunked Transfer-Encoding works end-to-end.
**Method:**
- Client sends POST with chunked body (no Content-Length)
- Server receives and echoes body back
- Verify response body matches request body exactly

**Acceptance Criteria:**
- [ ] Chunked request body (1KB, 10KB, 100KB) round-trips correctly
- [ ] Response chunked body (1KB, 10KB, 100KB) decodes correctly
- [ ] Multiple chunk boundaries handled correctly
- [ ] Chunk extensions (e.g., `5;ext=val\r\n`) handled correctly

### 7.5 Keep-Alive / Connection Reuse Test

**Requirement:** Measure connection setup overhead vs reuse overhead.
**Method:**
- Measure time for: (a) new connection + request + response, (b) reused connection + request + response
- Compare: single connection N sequential requests vs N separate connections
- Measure: connection setup cost, per-request overhead with reuse

**Acceptance Criteria:**
- [ ] Connection reuse saves >50% overhead vs new connection
- [ ] No data leakage between requests on reused connection
- [ ] Server correctly handles multiple requests per connection

### 7.6 Header Limit Boundary Test

**Requirement:** Verify header limits work correctly at all configured thresholds.
**Method:**
- Send requests with 16, 32, 64, 128, 256 headers
- Test with HttpServer (default=16), HttpServerWithHeaders<N=32>, N=64, N=128
- Measure: request acceptance/rejection, latency, memory usage

**Acceptance Criteria:**
- [ ] Requests at or below limit are accepted without error
- [ ] Requests above limit return 431 or 400 error
- [ ] No memory allocation above baseline at any limit
- [ ] Large header values (2KB+) at all limits handled correctly

### 7.7 All HTTP Verbs E2E Test

**Requirement:** Verify all HTTP verbs work correctly via native HttpClient.
**Method:**
- Server supports GET, POST, PUT, DELETE, PATCH, HEAD, OPTIONS
- Client sends each verb with appropriate request/response body
- Verify response status codes and body content

**Acceptance Criteria:**
- [ ] Each verb produces correct status code (200, 201, 204, etc.)
- [ ] Each verb correctly handles body presence/absence
- [ ] No verb causes unexpected errors or panics

### 7.8 Timeout Behavior Test

**Requirement:** Verify HttpClient::set_timeout() triggers correctly.
**Method:**
- Set 100ms timeout on client
- Server delays response by 500ms
- Verify client receives timeout error (not hang)

**Acceptance Criteria:**
- [ ] Timeout error returned within timeout + 10% margin
- [ ] Client does not hang on timeout
- [ ] Connection cleaned up after timeout

### 7.9 Slow Client / Buffer Drain Test

**Requirement:** Verify server handles slow clients without memory issues.
**Method:**
- Client sends data byte-by-byte (1 byte per read)
- Server must buffer and parse headers without filling memory
- Measure: server memory growth, parsing correctness, error handling

**Acceptance Criteria:**
- [ ] Server handles 1000-byte-per-request slow clients without OOM
- [ ] Request parsing succeeds or fails cleanly (no partial parsing)
- [ ] Memory usage stable over 100 slow client requests

### 7.10 Large Response Body Test

**Requirement:** Verify client reads large responses efficiently.
**Method:**
- Server sends responses of 100B, 1KB, 10KB, 100KB, 1MB, 10MB
- Client reads full response body
- Measure: read throughput (MB/s), memory usage, time to complete

**Acceptance Criteria:**
- [ ] All response sizes read correctly (content integrity verified)
- [ ] Throughput >= 50 MB/s for bodies >= 100KB
- [ ] Memory usage proportional to body size (no leaks)

---

## 8. TEST ARCHITECTURE RECOMMENDATION

### 8.1 File Structure

```
tests/
  performance/
    mod.rs                    — Test fixture setup, shared timing harness
    body_throughput.rs        — Req 7.1 (body size scaling)
    connection_scaling.rs     — Req 7.2 (concurrent connections)
    pipeline.rs               — Req 7.3 (pipelined requests)
    chunked_e2e.rs            — Req 7.4 (chunked round-trip)
    keepalive.rs              — Req 7.5 (connection reuse)
    header_boundaries.rs      — Req 7.6 (header limits)
    http_verbs.rs             — Req 7.7 (all verbs)
    timeout.rs                — Req 7.8 (timeout behavior)
    slow_client.rs            — Req 7.9 (buffer drain)
    large_response.rs         — Req 7.10 (large body)
  load/
    server_throughput.rs      — Goose load tests for throughput
    concurrent_load.rs        — Goose load tests with multiple users
```

### 8.2 Design Principles

1. **RAII Fixture Pattern** — Server starts on port 0 (random), stops on Drop
2. **Timing Harness** — Wrap request/response with `std::time::Instant` for latency tracking
3. **Goose Integration** — Use goose for multi-user concurrency tests
4. **Native Client Only** — Use `may_minihttp::client::HttpClient` (not may_http)
5. **No External Dependencies** — All tests self-contained, no live database required
6. **CI-Parity** — Tests must run in Docker container without network access

### 8.3 Example Fixture Pattern

```rust
use std::net::SocketAddr;
use std::sync::Arc;
use may_minihttp::{HttpServer, HttpService, HttpServerWithHeaders, Request, Response};
use may;

pub struct TestServer {
    addr: SocketAddr,
    handle: std::thread::JoinHandle<()>,
}

impl TestServer {
    pub fn new<F: HttpService + Clone + Send + Sync + 'static>(service: F) -> Self {
        // Bind to port 0 for random port
        let server = HttpServer(service);
        let handle = server.start("127.0.0.1:0").unwrap();
        let addr = /* extract from listener... */;
        Self { addr, handle }
    }

    pub fn addr(&self) -> SocketAddr { self.addr }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        // Signal shutdown, join handle
    }
}
```

---

## 9. PRIORITY MATRIX

| Priority | Test | Effort | Value | Reason |
|----------|------|--------|-------|--------|
| P0 | 7.1 Body size throughput | Medium | HIGH | Critical for production deployment decisions |
| P0 | 7.2 Concurrent connection scaling | Medium | HIGH | Determines scalability limits |
| P0 | 7.4 Chunked E2E | Low | HIGH | Currently untested protocol feature |
| P1 | 7.6 Header boundary test | Low | HIGH | Security-relevant (header injection) |
| P1 | 7.7 All HTTP verbs via native client | Low | HIGH | Native client support untested |
| P1 | 7.5 Keep-alive connection reuse | Medium | MEDIUM | Establishes baseline for future work |
| P2 | 7.3 Pipeline requests | Medium | MEDIUM | Server feature unmeasured |
| P2 | 7.10 Large response body | Medium | MEDIUM | Client read path unmeasured |
| P2 | 7.8 Timeout behavior | Low | MEDIUM | Error path verification |
| P3 | 7.9 Slow client / buffer drain | Low | LOW | Edge case |
| P3 | 7.9 Malformed request E2E | Low | LOW | Coverage completeness |

---

## 11. BENCHMARK RESULTS (Post-Implementation)

### 11.1 Throughput Benchmarks

| Test | Scenario | Result |
|------|----------|--------|
| POST body throughput | 200 POSTs, ~24B body | ~3,550 req/s (~0.09 MB/s) |
| Keep-alive fresh | 100 connections, GET | ~3,020 req/s |
| Keep-alive reused | 1 connection, 100 GETs | ~8,175 req/s (2.7x speedup) |
| Pipelined GET | 100 GETs pipelined | ~4,117 req/s |
| All-verb GET | 100 iterations | ~3,436 req/s |
| All-verb POST | 100 iterations | ~3,560 req/s |
| All-verb PUT | 100 iterations | ~3,461 req/s |
| All-verb DELETE | 100 iterations | ~2,852 req/s |
| All-verb PATCH | 100 iterations | ~3,763 req/s |

### 11.2 Connection Overhead

| Metric | Fresh Connection | Reused Connection | Savings |
|--------|-----------------|-------------------|---------|
| Throughput | ~3,020 req/s | ~8,175 req/s | 63% |
| Total time | ~33.1ms | ~12.2ms | 2.7x |

### 11.3 Latency Benchmarks

| Scenario | P50 Estimate | Notes |
|----------|-------------|-------|
| Simple GET (keep-alive) | <1ms | Single connection, sequential |
| POST with small body | <2ms | ~24B body, one connection |

### 11.4 Large Body Tests

| Size | Method | Result |
|------|--------|--------|
| 1B | POST echo | OK |
| 100B | POST echo | OK |
| 1KB | POST echo | OK |
| 4KB | POST echo | OK |
| 4,097B | POST echo | OK (boundary) |
| 8KB | POST echo | OK |
| 16KB | POST echo | OK |
| 32KB | POST echo | OK |
| 100KB | POST echo | OK |
| 1MB | Response read | OK |

### 11.5 Timeout Tests

| Scenario | Expected | Actual |
|----------|----------|--------|
| Read timeout (100ms) | Triggers error | Triggers after ~500ms (500ms server delay) |
| Write timeout (100ms) | Triggers error | Triggers correctly |
| Recovery after timeout | New request succeeds | Succeeds |
| Zero timeout (disabled) | No false error | No error |

### 11.6 Malformed Request Handling

| Scenario | Behavior |
|----------|----------|
| Service error (500) | Server returns 500, stays stable |
| 5 consecutive errors | Server stable, no crash |
| Recovery after error | Normal requests succeed |
| Raw TCP garbage | Server logs parse error, connection closed |
| Content-Length mismatch | Server processes what it receives |
| Garbage header values | Server accepts/rejects per limits |

### 11.7 Slow Client Resilience

| Scenario | Body Size | Result |
|----------|-----------|--------|
| 1-byte write chunks GET | 0B | OK |
| 1-byte write chunks POST | 500B | OK |
| 16-byte write chunks POST | 5KB | OK |
| Sequential 10 POSTs on 1 conn | 4B each | OK |
| 16 custom headers | 0B | OK |
| Large body (100KB) | 100KB | OK |

---

## 12. UPDATED COVERAGE SUMMARY

### What's Now Tested (E2E)
- Body size scaling: 1B–100KB POST round-trips with server-side counters (perf_chunked_e2e.rs)
- Concurrent connections: 100-fresh vs 1-reused connection overhead comparison (perf_keepalive.rs)
- Pipelining: 20 sequential GETs/POSTs on 1 connection, 100-request throughput (perf_pipelining.rs)
- Chunked encoding: Client POST chunked bodies, server echo round-trip (perf_chunked_e2e.rs)
- Connection reuse: Fresh vs keep-alive throughput, body integrity (perf_keepalive.rs)
- Timeout enforcement: Read/write timeout, recovery, disabled timeout (perf_timeout.rs)
- Slow client resilience: Small-write chunks, 100KB body, sequential on one connection (perf_slow_client.rs)
- All HTTP verbs: GET/POST/PUT/DELETE/PATCH/HEAD/OPTIONS with echo service (perf_all_verbs.rs)
- Large response reading: 1B–32KB across buffer boundaries, integrity verification (perf_large_response.rs)
- Malformed request handling: Service errors, header limits, raw socket garbage (perf_malformed.rs)
- Concurrent multi-client: 8×50 GET scaling (22k req/s), 200-client stress (168k req/s), mixed GET/POST/PUT (perf_concurrent_multi.rs)
- HTTP/1.0 wire format: 200/404/500 responses, custom headers, no Content-Length (perf_http10.rs)
- Malformed response E2E: Truncated bodies, invalid CL, duplicate headers, huge Content-Length, null bytes (perf_malformed_response.rs)
- Memory profiling: RSS deltas under sustained load, per-connection budget (< 64 KB), body-size leak detection, connection-churn stability convergence (perf_memory.rs)

### Remaining Uncovered
- **Windows platform**: No CI runner available for Windows testing
