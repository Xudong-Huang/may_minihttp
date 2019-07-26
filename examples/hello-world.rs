use may_minihttp::{HttpServer, HttpService, Request, Response};
use std::io;

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
    env_logger::init();
    let server = HttpServer(HelloWorld).start("127.0.0.1:8080").unwrap();
    server.wait();
}
