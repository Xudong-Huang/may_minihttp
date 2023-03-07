use may_minihttp::{HttpServer, HttpService, Request, Response};
use yarte::Serialize;

use std::io;

const JSON: &[u8] =
    b"HTTP/1.1 200 OK\r\nServer: M\r\nContent-Type: application/json\r\nContent-Length: 27\r\n";

#[derive(Serialize)]
struct HelloMessage {
    message: &'static str,
}

#[derive(Clone)]
struct HelloJson;

impl HttpService for HelloJson {
    fn call(&mut self, _req: Request, rsp: &mut Response) -> io::Result<()> {
        rsp.fixed_header(JSON);
        HelloMessage {
            message: "Hello, World!",
        }
        .to_bytes_mut(rsp.body_mut());
        Ok(())
    }
}

fn main() {
    let server = HttpServer(HelloJson).start("127.0.0.1:8080").unwrap();
    server.wait();
}
