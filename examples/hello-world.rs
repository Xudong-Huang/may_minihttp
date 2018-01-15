extern crate env_logger;

use std::io;

use may_minihttp::{Http, Request, Response};

/// `HelloWorld` is the *service* that we're going to be implementing to service
/// the HTTP requests we receive.
///
struct HelloWorld;

impl Service for HelloWorld {
    fn call(&self, _request: Request) -> io::Result<Response> {
        let mut resp = Response::new();
        resp.body("Hello, world!");
        Ok(resp)
    }
}

fn main() {
    drop(env_logger::init());
    let addr = "0.0.0.0:8080".parse().unwrap();
    TcpServer::new(Http, addr).serve(|| Ok(HelloWorld));
}
