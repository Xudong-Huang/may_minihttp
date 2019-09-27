use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use may_minihttp::{BodyWriter, HttpService, HttpServiceFactory, Request, Response};
use may_postgres::{self, Client, Statement};
use oorandom::Rand32;

use serde_derive::Serialize;

#[derive(Serialize)]
struct WorldRow {
    id: i32,
    randomnumber: i32,
}

struct PgConnectionPool {
    idx: AtomicUsize,
    clients: Vec<Arc<PgConnection>>,
}

impl PgConnectionPool {
    fn new(db_url: &str, size: usize) -> PgConnectionPool {
        let mut clients = Vec::with_capacity(size);
        for _ in 0..size {
            let client = PgConnection::new(db_url);
            clients.push(Arc::new(client));
        }

        PgConnectionPool {
            idx: AtomicUsize::new(0),
            clients,
        }
    }

    fn get_connection(&self) -> (Arc<PgConnection>, usize) {
        let idx = self.idx.fetch_add(1, Ordering::Relaxed);
        let len = self.clients.len();
        (self.clients[idx % len].clone(), idx)
    }
}

struct PgConnection {
    client: Client,
    world: Statement,
}

unsafe impl Send for PgConnection {}

impl PgConnection {
    fn new(db_url: &str) -> Self {
        let client = may_postgres::connect(db_url).unwrap();
        let world = client
            .prepare("SELECT id, randomnumber FROM world WHERE id=$1")
            .unwrap();

        PgConnection { client, world }
    }

    fn get_world(&self, random_id: i32) -> Result<WorldRow, may_postgres::Error> {
        let mut rows = self.client.query(&self.world, &[&random_id]);
        let row = match rows.next() {
            Some(r) => r?,
            None => {
                dbg!(random_id);
                return Ok(WorldRow {
                    id: 0,
                    randomnumber: 0,
                });
            }
        };

        Ok(WorldRow {
            id: row.get(0),
            randomnumber: row.get(1),
        })
    }
}

struct Techempower {
    db: Arc<PgConnection>,
    rng: Rand32,
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
                let random_id = self.rng.rand_range(1..10001) as i32;
                let world = self
                    .db
                    .get_world(random_id)
                    .expect("failed to get random world");
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
    db_pool: PgConnectionPool,
}

impl HttpServiceFactory for HttpServer {
    type Service = Techempower;

    fn new_service(&self) -> Self::Service {
        let (db, idx) = self.db_pool.get_connection();
        let rng = Rand32::new(idx as u64);
        Techempower { db, rng }
    }
}

fn main() {
    let cpus = num_cpus::get();
    may::config()
        .set_io_workers(cpus)
        .set_workers(cpus)
        .set_pool_capacity(10000);
    let db_url = "postgres://benchmarkdbuser:benchmarkdbpass@127.0.0.1/hello_world";
    let http_server = HttpServer {
        db_pool: PgConnectionPool::new(db_url, cpus),
    };
    let server = http_server.start("127.0.0.1:8081").unwrap();
    server.join().unwrap();
}
