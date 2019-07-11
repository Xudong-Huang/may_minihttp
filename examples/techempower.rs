extern crate may;
extern crate may_minihttp;
#[macro_use]
extern crate serde_json;

use may_minihttp::{HttpServer, HttpService, Request, Response};
use std::io;

struct Techempower;

impl HttpService for Techempower {
    fn call(&self, req: Request) -> io::Result<Response> {
        let mut resp = Response::new();

        // Bare-bones router
        match req.path() {
            "/json" => {
                resp.header("Content-Type", "application/json");
                serde_json::to_writer(resp.body_mut(), &json!({"message": "Hello, World!"}))?;
            }
            "/plaintext" => {
                resp.header("Content-Type", "text/plain")
                    .body("Hello, World!");
            }
            _ => {
                resp.status_code("404", "Not Found");
            }
        }

        Ok(resp)
    }
}

fn main() {
    may::config().set_io_workers(4);
    let server = HttpServer(Techempower).start("127.0.0.1:8080").unwrap();
    server.join().unwrap();
}
