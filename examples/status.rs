extern crate env_logger;

use std::io;

use may_minihttp::{Http, Request, Response};

struct StatusService;

impl Service for StatusService {
    type Request = Request;
    type Response = Response;
    type Error = io::Error;
    type Future = future::Ok<Response, io::Error>;

    fn call(&self, _request: Request) -> Self::Future {
        let (code, message) = match _request.path() {
            "/200" => (200, "OK"),
            "/400" => (400, "Bad Request"),
            "/500" => (500, "Internal Server Error"),
            _ => (404, "Not Found"),
        };

        let mut resp = Response::new();
        resp.status_code(code, message);
        resp.body(message);
        future::ok(resp)
    }
}

fn main() {
    drop(env_logger::init());
    let addr = "0.0.0.0:8080".parse().unwrap();
    TcpServer::new(Http, addr).serve(|| Ok(StatusService));
}
