extern crate may;
extern crate may_minihttp;
#[macro_use]
extern crate serde_json;

use may_minihttp::{BodyWriter, HttpServer, HttpService, Request, Response};
use std::io;

struct HellorJson;

impl HttpService for HellorJson {
    fn call(&self, _request: Request) -> io::Result<Response> {
        let mut resp = Response::new();
        resp.header("Content-Type", "application/json");
        let w = BodyWriter(resp.body_mut());
        serde_json::to_writer(w, &json!({"message": "Hello, World!"}))?;
        Ok(resp)
    }
}

fn main() {
    may::config().set_io_workers(2);
    let server = HttpServer(HellorJson).start("127.0.0.1:8080").unwrap();
    server.wait();
}
