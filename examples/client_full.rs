//! Full request cycle: different HTTP methods and response inspection.
//!
//! Run with:
//!     cargo run --example client_full --features client
//!
//! Demonstrates:
//! - HttpClient::connect with timeout
//! - HttpClient::get for simple GET requests
//! - HttpClient::new_request + send_request for method-specific requests
//! - HEAD responses (no body, EmptyReader)
//! - PUT/PATCH with body
//! - DELETE without body

use std::io::Read;
use std::time::Duration;

use http::{Method, Uri};
use may_minihttp::client::HttpClient;

fn print_body(response: &mut impl Read) {
    let mut buf = [0u8; 8192];
    match response.read(&mut buf) {
        Ok(n) if n > 0 => {
            let body = String::from_utf8_lossy(&buf[..n]);
            for line in body.lines().take(10) {
                println!("    {}", line);
            }
            if body.lines().count() > 10 {
                println!("    ... (truncated)");
            }
        }
        _ => println!("    <empty>"),
    }
}

fn main() {
    env_logger::init();

    let mut client = HttpClient::connect("httpbin.org:443").expect("failed to connect");
    client.set_timeout(Some(Duration::from_secs(5)));

    println!("=== GET /get ===");
    let uri: Uri = "/get".parse().unwrap();
    let mut response = client.get(uri).expect("GET failed");
    println!(
        "  Status: {} {}",
        response.status().as_u16(),
        response.status().canonical_reason().unwrap_or("?")
    );
    print_body(&mut response);

    println!("\n=== HEAD /headers ===");
    let request = client.new_request(Method::HEAD, "/headers".parse().unwrap());
    let response = client.send_request(request).expect("HEAD failed");
    println!(
        "  Status: {} {}",
        response.status().as_u16(),
        response.status().canonical_reason().unwrap_or("?")
    );
    println!(
        "  Content-Type: {:?}",
        response.headers().get("content-type")
    );
    let mut response: may_minihttp::client::Response = response;
    print_body(&mut response);

    println!("\n=== PUT /put ===");
    let mut request = client.new_request(Method::PUT, "/put".parse().unwrap());
    *request.method_mut() = Method::PUT;
    *request.uri_mut() = "/put".parse().unwrap();
    request
        .send(b"\"hello world\"")
        .expect("failed to send PUT body");
    let mut response = client.send_request(request).expect("PUT failed");
    println!(
        "  Status: {} {}",
        response.status().as_u16(),
        response.status().canonical_reason().unwrap_or("?")
    );
    print_body(&mut response);

    println!("\n=== DELETE /delete ===");
    let request = client.new_request(Method::DELETE, "/delete".parse().unwrap());
    let mut response = client.send_request(request).expect("DELETE failed");
    println!(
        "  Status: {} {}",
        response.status().as_u16(),
        response.status().canonical_reason().unwrap_or("?")
    );
    print_body(&mut response);

    println!("\n=== PATCH /patch ===");
    let mut request = client.new_request(Method::PATCH, "/patch".parse().unwrap());
    *request.method_mut() = Method::PATCH;
    *request.uri_mut() = "/patch".parse().unwrap();
    request
        .send(b"{\"patched\": true}")
        .expect("failed to send PATCH body");
    let mut response = client.send_request(request).expect("PATCH failed");
    println!(
        "  Status: {} {}",
        response.status().as_u16(),
        response.status().canonical_reason().unwrap_or("?")
    );
    print_body(&mut response);
}
