extern crate env_logger;
extern crate may;
extern crate may_minihttp;

use std::io;
use may_minihttp::{HttpServer, HttpService, Request, Response};

/// `HelloWorld` is the *service* that we're going to be implementing to service
/// the HTTP requests we receive.
///
struct HelloWorld;

impl HttpService for HelloWorld {
    fn call(&self, _request: Request) -> io::Result<Response> {
        let mut resp = Response::new();
        resp.body("Hello, world!");
        Ok(resp)
    }
}

fn main() {
    may::config().set_io_workers(1);
    drop(env_logger::init());
    let server = HttpServer(HelloWorld).start("0.0.0.0:8080").unwrap();
    server.join().unwrap();
}
