extern crate may;
extern crate may_minihttp;
#[macro_use]
extern crate serde_json;

use std::io;
use may_minihttp::{HttpServer, HttpService, Request, Response};

struct HellorJson;

impl HttpService for HellorJson {
    fn call(&self, _request: Request) -> io::Result<Response> {
        let mut resp = Response::new();
        resp.header("Content-Type", "application/json");
        *resp.body_mut() = serde_json::to_vec(&json!({"message": "Hello, World!"})).unwrap();
        Ok(resp)
    }
}

fn main() {
    may::config().set_io_workers(2);
    let server = HttpServer(HellorJson).start("127.0.0.1:8080").unwrap();
    server.wait();
}
