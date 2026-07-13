# may_minihttp Client — Design Document

## Status (2026-07-09)

**Phase 1 shipped in-tree:** native HTTP/1.1 client under `may_minihttp::client`, feature-gated
(`client`). Drop-in compatible with `may_http::client::{HttpClient, Request, Response}` — BRRTRouter
migrates here; the abandoned `may_http` crate is retired.

Future phases (RequestBuilder, JSON helpers, middleware, pooling) remain on the roadmap below.

---

## Why a client in may_minihttp?

`may_http` (~6 years unmaintained) is a thin wrapper around `may::net::TcpStream`. `may_minihttp` is
actively maintained (same author: Xudong Huang) and already owns HTTP parsing for the server side.

Adding a **native** client here:

1. One crate for may HTTP server + client (Microscaler fork already in BRRTRouter / Sesame-IDAM)
2. Fixes like DELETE/PUT/PATCH `write_head` live in the maintained crate, not a dead fork
3. Server types (`request.rs`, `response.rs`) stay unchanged — client is a separate module
4. Feature-gated (`client`) so server-only builds are unchanged

## Current may_minihttp API

### Server (existing, unchanged)

```rust
HttpServer<Service>::start(addr) -> JoinHandle<()>
Request::method() / path() / headers() / body()
Response::status_code() / header() / body()
```

### Client (Phase 1 — `feature = "client"`)

```rust
use may_minihttp::client::{HttpClient, Request, Response};

let mut client = HttpClient::connect(("127.0.0.1", 8080))?;
client.set_timeout(Some(Duration::from_secs(30)));

// Shortcuts
let rsp = client.get("/health".parse()?)?;
let rsp = client.post("/api".parse()?, body_bytes)?;

// Full control (DELETE, PUT, PATCH, custom headers)
let mut req = client.new_request(Method::DELETE, "/fleet/42".parse()?);
// req.headers_mut().insert(...);
drop(req); // writes head on Drop
let rsp = client.send_request(req)?; // or get_rsp after drop(req)
```

API mirrors `may_http::client` so BRRTRouter `proxy.rs` / `fetch.rs` need only import path changes.

## Architecture

```
may_minihttp/src/
├── lib.rs
├── http_server.rs          # existing server
├── request.rs              # existing server request (httparse)
├── response.rs             # existing server response
└── client/                 # feature = "client"
    ├── mod.rs
    ├── buffer.rs           # BufferIo<TcpStream>
    ├── body/               # BodyReader, BodyWriter
    ├── client_impl.rs      # HttpClient
    ├── request.rs          # outgoing Request (http 0.2)
    └── response.rs         # incoming Response (httparse decode)
```

**No `may_http` dependency.** Transport is `may::net::TcpStream` + `BufferIo`, same wire format as
the former `may_http` client.

### DELETE / PUT / PATCH

`Request::write_head` handles all methods. Non-GET/HEAD/POST with no `Content-Length` assume an empty
body (no `Transfer-Encoding: chunked` for those methods). Call `set_content_length` before writing
when the body size is known.

Regression tests in `client/request.rs`.

## Dependencies

| Crate | Scope | Notes |
|-------|-------|-------|
| `http = "0.2"` | `client` feature only | Uri, Method, StatusCode, HeaderMap |
| `httparse`, `bytes`, `log`, `may` | always | shared with server |

Server continues to use `httparse` directly — no type mixing between server and client modules.

## Evolution Plan

### Phase 1: Core client (this PR) ✅

- Native `HttpClient`, `Request`, `Response` — `may_http`-compatible API
- GET, POST, DELETE, PUT, PATCH via `new_request` / `send_request`
- Feature gate `client`
- BRRTRouter migration off `may_http`

### Phase 2: Rich API (future)

- `RequestBuilder`, domain `Error` enum, JSON helpers (`serde_json` optional)
- PUT/DELETE/PATCH convenience methods on `HttpClient`
- Middleware trait

### Phase 3: Robustness (future)

- Connection pooling, retries, streaming bodies for large responses

### Phase 4: Production (future)

- TLS behind feature flag (BRRTRouter already uses rustls for HTTPS)
- Metrics, compression, HTTP/2 if `may` supports it

## Resolved Questions

1. **Separate crate vs in-tree?** → In `may_minihttp`, feature-gated.
2. **Replace `may_http`?** → Yes for Microscaler consumers (BRRTRouter). Fork archived after migration.
3. **`http` version?** → `0.2` in client module only; workspace `http 1.0` for BRRTRouter server types unchanged.
4. **Body buffering?** → Buffer via `Read` on `Response` (same as `may_http`).

## Open Questions

1. Upstream contribution to Xudong-Huang/may_minihttp — proceed locally; upstream ping optional.
2. Rich client API (Phase 2) vs minimal compat layer — compat first, richness when Sesame-IDAM needs it.
