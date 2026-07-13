//! HEAD request using the native HTTP/1.1 client.
//!
//! Run with:
//!     cargo run --example client_head --features client
//!
//! Demonstrates:
//! - HttpClient::new_request with Method::HEAD
//! - HttpClient::send_request
//! - HEAD responses have no body (EmptyReader)
//! - Accessing only headers

use std::io::Read;

use http::Method;
use may_minihttp::client::{HttpClient, Response};

fn main() {
    env_logger::init();

    let mut client = HttpClient::connect("httpbin.org:443").expect("failed to connect");

    // HEAD requests use new_request + send_request.
    // The client automatically sets expect_body(false) for HEAD,
    // so Response::set_reader selects EmptyReader and avoids
    // an infinite block waiting for a body that never comes.
    let uri = "/headers".parse().expect("invalid URI");
    let request = client.new_request(Method::HEAD, uri);
    let response = client.send_request(request).expect("request failed");
    let status = response.status();

    println!("Status: {}", status);
    println!("Content-Type: {:?}", response.headers().get("content-type"));
    println!(
        "Content-Length: {:?}",
        response.headers().get("content-length")
    );
    println!("Date: {:?}", response.headers().get("date"));

    // The body is an EmptyReader — read returns 0 immediately.
    let mut buf = [0u8; 256];
    let mut response: Response = response;
    let n = response.read(&mut buf).expect("read failed");
    println!("\nBody bytes read: {} (HEAD responses have no body)", n);
}
