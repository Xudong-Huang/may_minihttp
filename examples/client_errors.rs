//! Error handling and timeout configuration.
//!
//! Run with:
//!     cargo run --example client_errors --features client
//!
//! Demonstrates:
//! - Connection errors (unreachable host)
//! - Timeout handling
//! - Inspecting io::Error details

use std::io;
use std::time::Duration;

use may_minihttp::client::HttpClient;

fn main() {
    env_logger::init();

    println!("=== Connection Error Example ===");
    // Connect to a port with no server — expect connection refused.
    match HttpClient::connect("127.0.0.1:19999") {
        Ok(_) => println!("  Unexpected: connection succeeded"),
        Err(e) => {
            println!("  Connection error: {}", e);
            println!("  Kind: {:?}", e.kind());
            println!("  Expected: ConnectionRefused");
        }
    }

    println!("\n=== Timeout Example ===");
    // Connect to a real server with a very short timeout.
    // Note: httpbin.org requires TLS, so direct TCP connect to port 443
    // will work but the HTTP response may be TLS-garbled.
    // The timeout itself is what we're demonstrating.
    match HttpClient::connect("127.0.0.1:8080") {
        Ok(_) => println!("  Unexpected: connected to port 8080"),
        Err(e) => {
            println!("  Connection error: {}", e);
            println!("  Kind: {:?}", e.kind());
        }
    }

    // Show how to set timeouts on a connected client.
    // Note: EOPNOTSUPP may be returned on non-blocking sockets —
    // this is expected and silently ignored by set_timeout.
    {
        // Use an unreachable address to demonstrate error handling
        // when setting timeouts on a hypothetical connection.
        match HttpClient::connect("127.0.0.1:19998") {
            Ok(mut client) => {
                client.set_timeout(Some(Duration::from_millis(100)));
                println!("  Timeout set (may return EOPNOTSUPP silently)");
            }
            Err(e) => println!("  Cannot connect to set timeout: {}", e),
        }
    }

    println!("\n=== Error Kind Reference ===");
    for kind in [
        io::ErrorKind::ConnectionRefused,
        io::ErrorKind::TimedOut,
        io::ErrorKind::UnexpectedEof,
        io::ErrorKind::InvalidInput,
    ] {
        let e = io::Error::new(kind, "example");
        println!("  {:?}: {}", kind, e);
    }
}
