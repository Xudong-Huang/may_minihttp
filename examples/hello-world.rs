use may_minihttp::{HttpServer, HttpService, Request, Response};
use std::io;

/// `HelloWorld` is the *service* that we're going to be implementing to service
/// the HTTP requests we receive.
///
#[derive(Clone)]
struct HelloWorld;

impl HttpService for HelloWorld {
    fn call(&mut self, _req: Request, rsp: &mut Response) -> io::Result<()> {
        rsp.body("Hello, world!");
        Ok(())
    }
}

fn main() {
    env_logger::init();
    let server = HttpServer(HelloWorld).start("127.0.0.1:8080").unwrap();
    server.wait();
}
