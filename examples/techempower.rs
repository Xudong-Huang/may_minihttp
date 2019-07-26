use std::io;
use std::pin::Pin;

use may_minihttp::{BodyWriter, HttpService, HttpServiceFactory, Request, Response};
use postgres::{stmt::Statement, Connection, TlsMode};
use rand::{rngs::ThreadRng, Rng};

use serde_derive::Serialize;

#[derive(Serialize)]
struct WorldRow {
    id: i32,
    randomnumber: i32,
}

struct PgConnection {
    con: Pin<Box<Connection>>,
    // fortune: Statement<'static>,
    world: Statement<'static>,
    rng: ThreadRng,
}

unsafe impl Send for PgConnection {}

impl PgConnection {
    fn new(db_url: &str) -> Self {
        let con = Box::pin(Connection::connect(db_url, TlsMode::None).unwrap());
        let rng = rand::thread_rng();
        let mut db = PgConnection {
            con,
            rng,
            world: unsafe { std::mem::MaybeUninit::uninit().assume_init() },
        };

        let world = db
            .con
            .prepare("SELECT id, randomnumber FROM world WHERE id=$1")
            .unwrap();
        db.world = unsafe { std::mem::transmute(world) };
        db
    }

    fn get_world(&mut self) -> io::Result<WorldRow> {
        let random_id = self.rng.gen_range(1, 10_001);
        let rows = self.world.query(&[&random_id])?;
        let row = rows.get(0);

        Ok(WorldRow {
            id: row.get(0),
            randomnumber: row.get(1),
        })
    }
}

struct Techempower {
    db: PgConnection,
}

impl HttpService for Techempower {
    fn call(&mut self, req: Request) -> io::Result<Response> {
        let mut resp = Response::new();

        // Bare-bones router
        match req.path() {
            "/json" => {
                resp.header("Content-Type", "application/json");
                let body = resp.body_mut();
                body.reserve(27);
                let w = BodyWriter(body);
                serde_json::to_writer(w, &serde_json::json!({"message": "Hello, World!"}))?;
            }
            "/plaintext" => {
                resp.header("Content-Type", "text/plain")
                    .body("Hello, World!");
            }
            "/db" => {
                let world = self.db.get_world().expect("failed to get random world");
                resp.header("Content-Type", "application/json");
                let body = resp.body_mut();
                let w = BodyWriter(body);
                serde_json::to_writer(w, &world)?;
            }
            _ => {
                resp.status_code("404", "Not Found");
            }
        }

        Ok(resp)
    }
}

struct HttpServer {
    db_url: String,
}

impl HttpServiceFactory for HttpServer {
    type Service = Techempower;

    fn new_service(&self) -> Self::Service {
        let db = PgConnection::new(&self.db_url);
        Techempower { db }
    }
}

fn main() {
    may::config().set_io_workers(num_cpus::get());
    let db_url = "postgres://benchmarkdbuser:benchmarkdbpass@tfb-database/hello_world";
    let http_server = HttpServer {
        db_url: db_url.to_owned(),
    };
    let server = http_server.start("127.0.0.1:8080").unwrap();
    server.join().unwrap();
}
