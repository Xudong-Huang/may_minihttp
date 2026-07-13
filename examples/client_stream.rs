//! Streaming body read with chunked transfer encoding.
//!
//! Run with:
//!     cargo run --example client_stream --features client
//!
//! Demonstrates:
//! - Streaming body read with Read trait
//! - Handling large responses without loading into memory
//! - Chunked transfer encoding support

use std::io::{self, Read};

use may_minihttp::client::HttpClient;

/// Read the response body in chunks, printing each chunk.
fn stream_body(mut response: impl Read) -> io::Result<usize> {
    let mut buf = [0u8; 4096];
    let mut total = 0;

    loop {
        match response.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                total += n;
                let chunk = String::from_utf8_lossy(&buf[..n]);
                print!("  [{} bytes] {}", n, chunk);
            }
            Err(e) => {
                eprintln!("\nError: {}", e);
                break;
            }
        }
    }

    println!("  Total: {} bytes", total);
    Ok(total)
}

fn main() {
    env_logger::init();

    let mut client = HttpClient::connect("httpbin.org:443").expect("failed to connect");

    // The /bytes endpoint returns random bytes with Content-Length.
    let uri = "/bytes/4096".parse().expect("invalid URI");
    let response = client.get(uri).expect("request failed");

    println!("Status: {}", response.status());
    println!(
        "Content-Length: {:?}",
        response.headers().get("content-length")
    );

    // Stream the body in 4KB chunks.
    stream_body(response).expect("streaming failed");
}
