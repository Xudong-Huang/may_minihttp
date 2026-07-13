//! POST request with body.
//!
//! Demonstrates sending a POST request with a body using the `post()`
//! convenience method, and also shows the `new_request()` API for more
//! control.

use std::io::Write;

use http::Method;
use may_minihttp::client::{HttpClient, Request};

fn main() {
    // Connect to the server
    let mut client = HttpClient::connect("127.0.0.1:8080").expect("failed to connect");

    // --- Convenience method: post() ---
    // Note: post() requires a type implementing bytes::Buf (&[u8] works,
    // &[u8; N] does not — use &bytes[..] instead).
    let mut response = client
        .post(
            "/submit".parse().expect("invalid uri"),
            &b"Hello, World!"[..],
        )
        .expect("POST request failed");
    println!("POST (convenience): {}", response.status());

    // Read and discard the response
    let _body = read_body(&mut response);

    // --- Explicit method: new_request() + send_request() ---
    let mut request: Request =
        client.new_request(Method::POST, "/api/data".parse().expect("invalid uri"));
    request
        .headers_mut()
        .append("Content-Type", "application/json".parse().unwrap());
    request
        .headers_mut()
        .append("X-Custom", "my-value".parse().unwrap());

    request
        .body_mut()
        .write_all(b"{\"key\": \"value\"}")
        .unwrap();

    let response = client.send_request(request).expect("request failed");
    println!("POST (explicit):  {}", response.status());

    for (key, value) in response.headers() {
        println!("  {} => {}", key, value.to_str().unwrap_or("?"));
    }
}

fn read_body(response: &mut impl std::io::Read) -> String {
    let mut body = String::new();
    std::io::Read::read_to_string(response, &mut body).unwrap_or_default();
    body
}
