use may_minihttp::{BodyWriter, HttpServer, HttpService, Request, Response};
use std::io;

#[derive(Clone)]
struct HelloJson;

impl HttpService for HelloJson {
    fn call(&mut self, _request: Request) -> io::Result<Response> {
        let mut resp = Response::new();
        resp.header("Content-Type", "application/json");
        let w = BodyWriter(resp.body_mut());
        serde_json::to_writer(w, &serde_json::json!({"message": "Hello, World!"}))?;
        Ok(resp)
    }
}

fn main() {
    may::config().set_io_workers(2);
    let server = HttpServer(HelloJson).start("127.0.0.1:8080").unwrap();
    server.wait();
}
