use may_minihttp::{BodyWriter, HttpServer, HttpService, Request, Response};
use std::io::{self, Read};

#[derive(Clone)]
struct HelloJson;

impl HttpService for HelloJson {
    fn call(&mut self, mut req: Request, rsp: &mut Response) -> io::Result<()> {
        let value: serde_json::Value = serde_json::from_reader(&mut req.body)?;
        rsp.header("Content-Type: application/json");
        let w = BodyWriter(rsp.body_mut());
        if value
            .as_object()
            .unwrap()
            .get("token")
            .unwrap()
            .as_str()
            .unwrap()
            == "LOmCXi7MkpRozLJvLrK6fA=="
        {
            serde_json::to_writer(w, &serde_json::json!({ "status": "ok" }))?;
        } else {
            serde_json::to_writer(w, &serde_json::json!({ "status": "denied" }))?;
        }

        Ok(())
    }
}

fn main() {
    let server = HttpServer(HelloJson).start("127.0.0.1:8080").unwrap();
    server.wait();
}
