# Epic 01: Core Client

## Overview

Native HTTP/1.1 client for `may_minihttp`: connection management, request/response types, and
`may_http::client`-compatible API. No dependency on the abandoned `may_http` crate.

**Status:** IN PROGRESS (Phase 1 — compat layer landed 2026-07-09)

**Target Milestone:** Release 0.2.0 (Microscaler fork)

**Dependencies:** None

---

## Stories

| Story | Title | Status |
|-------|-------|--------|
| [01.1](stories/01.1-project-setup.md) | Project Setup & Cargo.toml | DONE |
| [01.2](stories/01.2-error-types.md) | Error Types | DEFERRED (Phase 2 — uses `io::Result` in Phase 1) |
| [01.3](stories/01.3-http-client.md) | HttpClient: Connection & Configuration | DONE |
| [01.4](stories/01.4-request-builder.md) | RequestBuilder: GET/POST with Headers | DEFERRED (Phase 2) |
| [01.5](stories/01.5-response.md) | Response: Status, Headers, Body, JSON | DONE (compat `Response`) |
| [01.6](stories/01.6-integration-tests.md) | Integration Tests with Mock Server | TODO |

---

## Definition of Done (Epic-level)

- [x] Native client module under `src/client/`, feature-gated
- [x] `cargo check --features client` passes
- [x] DELETE/PUT/PATCH do not panic in `Request::Drop`
- [x] BRRTRouter migrated off `may_http`
- [ ] Integration test against in-process `HttpServer`
- [ ] `docs/design-http-client.md` reflects shipped architecture
- [ ] Server-side code unchanged without `client` feature

---

## Risks

| Risk | Mitigation |
|------|-----------|
| Breaking server API | Client in `src/client/`, feature-gated |
| `http 0.2` vs `1.0` conflict | `http 0.2` only in `client/` module |
| Upstream may_minihttp drift | Microscaler fork branch `integration/microscaler-fork` |
