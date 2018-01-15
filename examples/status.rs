extern crate env_logger;

use std::io;

use may_minihttp::{HttpServer, Request, Response};

struct StatusService;

impl Service for StatusService {
    fn call(&self, _request: Request) -> io::Result<Response> {
        let (code, message) = match _request.path() {
            "/200" => (200, "OK"),
            "/400" => (400, "Bad Request"),
            "/500" => (500, "Internal Server Error"),
            _ => (404, "Not Found"),
        };

        let mut resp = Response::new();
        resp.status_code(code, message);
        resp.body(message);
        Ok(resp)
    }
}

fn main() {
    drop(env_logger::init());
    let server = HttpServer(StatusService).start("0.0.0.0:8080").unwrap();
    server.join().unwrap();
}
