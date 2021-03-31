use std::io;

use may_minihttp::{HttpService, HttpServiceFactory, Request, Response};
use yarte::Serialize;

#[derive(Serialize)]
struct HeloMessage {
    message: &'static str,
}

struct Techempower {}

impl HttpService for Techempower {
    fn call(&mut self, req: Request, rsp: &mut Response) -> io::Result<()> {
        // Bare-bones router
        match req.path() {
            "/json" => {
                rsp.header("Content-Type: application/json");
                HeloMessage {
                    message: "Hello, World!",
                }
                .to_bytes_mut(rsp.body_mut());
            }
            "/plaintext" => {
                rsp.header("Content-Type: text/plain").body("Hello, World!");
            }
            _ => {
                rsp.status_code("404", "Not Found");
            }
        }

        Ok(())
    }
}

struct HttpServer {}

impl HttpServiceFactory for HttpServer {
    type Service = Techempower;

    fn new_service(&self) -> Self::Service {
        Techempower {}
    }
}

fn main() {
    may::config()
        .set_pool_capacity(10000)
        .set_stack_size(0x1000);
    let http_server = HttpServer {};
    let server = http_server.start("0.0.0.0:8081").unwrap();
    server.join().unwrap();
}
