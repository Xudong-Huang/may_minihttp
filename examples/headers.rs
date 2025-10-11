use bytes::BufMut;
use may_minihttp::{HttpServerWithHeaders, HttpService, Request, Response};
use std::io::{self, Write};

/// Example service that echoes back the number of request headers received.
/// Demonstrates handling requests with more than the default 16 headers.
#[derive(Clone)]
struct HeaderEcho;

impl HttpService for HeaderEcho {
    fn call(&mut self, req: Request, rsp: &mut Response) -> io::Result<()> {
        let headers = req.headers();
        let mut w = rsp.body_mut().writer();

        writeln!(w, "Received {} headers:\n", headers.len())?;
        for header in headers {
            writeln!(
                w,
                "{}: {}",
                header.name,
                std::str::from_utf8(header.value).unwrap_or("<invalid utf8>")
            )?;
        }
        Ok(())
    }
}

fn main() {
    env_logger::init();

    // HttpServerWithHeaders<Service, N> allows configuring max headers
    // Here we use 32 headers to handle modern browser/proxy traffic
    // Bind to 0.0.0.0 to allow connections from outside the container
    let bind_addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:8081".to_string());
    let server = HttpServerWithHeaders::<_, 32>(HeaderEcho)
        .start(&bind_addr)
        .unwrap();

    println!("Server listening on http://{}", bind_addr);
    println!("Configured to accept up to 32 headers");
    server.wait();
}
