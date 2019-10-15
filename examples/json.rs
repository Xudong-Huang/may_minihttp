use may_minihttp::{HttpServer, HttpService, Request, Response};
use std::io;

#[derive(Clone)]
struct HelloJson;

impl HttpService for HelloJson {
    fn call(&mut self, _req: Request, rsp: &mut Response) -> io::Result<()> {
        rsp.header("Content-Type: application/json");
        let body = rsp.get_body();
        serde_json::to_writer(body, &serde_json::json!({"message": "Hello, World!"}))?;
        Ok(())
    }
}

fn main() {
    may::config().set_io_workers(2);
    let server = HttpServer(HelloJson).start("127.0.0.1:8080").unwrap();
    server.wait();
}
