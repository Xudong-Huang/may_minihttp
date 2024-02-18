use may_minihttp::{BodyWriter, HttpServer, HttpService, Request, Response};

#[derive(Clone)]
struct HelloJson;

impl HttpService for HelloJson {
    fn call(&mut self, mut req: Request, rsp: &mut Response) -> std::io::Result<()> {
        // println!("req: {:?}", std::str::from_utf8(req.body()).unwrap());
        let value: serde_json::Value = serde_json::from_reader(&mut req)?;
        println!("value: {:?}", value);
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

// curl -v -X POST http://127.0.0.1:8080 -H 'Content-Type: application/json' -d '{"token":"LOmCXi7MkpRozLJvLrK6fA=="}'
fn main() {
    let server = HttpServer(HelloJson).start("127.0.0.1:8080").unwrap();
    server.wait();
}
