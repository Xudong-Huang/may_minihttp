use bytes::BufMut;
use may_minihttp::{HttpServer, HttpService, Request, Response};

#[derive(Clone)]
struct HelloJson;

impl HttpService for HelloJson {
    fn call(&mut self, req: Request, rsp: &mut Response) -> std::io::Result<()> {
        let method = req.method();
        println!("method: {:?}", method);
        let body = req.body();
        println!("body_limit: {:?}", body.body_limit());
        let value: serde_json::Value = serde_json::from_reader(body)?;
        println!("value: {:?}", value);
        rsp.header("Content-Type: application/json");
        let w = rsp.body_mut().writer();
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

// curl -v -X POST http://127.0.0.1:8080 -H 'Content-Type: application/json' -d '{"token":"LOmCXi7MkpRozLJvLrK6fA=="}'
fn main() {
    let server = HttpServer(HelloJson).start("127.0.0.1:8080").unwrap();
    server.wait();
}
