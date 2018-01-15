extern crate may;
extern crate may_minihttp;
#[macro_use]
extern crate serde_json;

use std::io;
use may_minihttp::{HttpServer, HttpService, Request, Response};

struct Techempower;

impl HttpService for Techempower {
    fn call(&self, req: Request) -> io::Result<Response> {
        let mut resp = Response::new();

        // Bare-bones router
        match req.path() {
            "/json" => {
                let json = serde_json::to_string(&json!({"message": "Hello, World!"})).unwrap();
                resp.header("Content-Type", "application/json").body(&json);
            }
            "/plaintext" => {
                resp.header("Content-Type", "text/plain")
                    .body("Hello, World!");
            }
            _ => {
                resp.status_code(404, "Not Found");
            }
        }

        Ok(resp)
    }
}

fn main() {
    may::config().set_io_workers(4);
    let server = HttpServer(Techempower).start("0.0.0.0:8080").unwrap();
    server.join().unwrap();
}
