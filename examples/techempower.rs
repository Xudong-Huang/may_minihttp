extern crate may;
extern crate may_minihttp;
extern crate serde_json;

use may_minihttp::{HttpServer, Request, Response};
use serde_json::builder::ObjectBuilder;

struct Techempower;

impl Service for Techempower {
    fn call(&self, req: Request) -> io::Result<Response> {
        let mut resp = Response::new();

        // Bare-bones router
        match req.path() {
            "/json" => {
                let json = serde_json::to_string(&ObjectBuilder::new()
                    .insert("message", "Hello, World!")
                    .build())
                    .unwrap();

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
    let server = HttpServer(StatusService).start("0.0.0.0:8080").unwrap();
    server.join().unwrap();
}
