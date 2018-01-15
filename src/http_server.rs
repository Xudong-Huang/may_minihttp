use std::io::{self, Read, Write};
use std::net::ToSocketAddrs;
use std::sync::Arc;

use bytes::{BufMut, BytesMut};
use co_managed::Manager;
use may::coroutine;
use may::net::TcpListener;

use request::{self, Request};
use response::{self, Response};

pub trait HttpService {
    fn call(&self, _request: Request) -> io::Result<Response>;
}

// this is a kind of server
pub struct HttpServer<T>(pub T);

macro_rules! t {
    ($e: expr) => (match $e {
        Ok(val) => val,
        Err(err) => {
            error!("call = {:?}\nerr = {:?}", stringify!($e), err);
            continue;
        }
    })
}

impl<T: HttpService + Send + Sync + 'static> HttpServer<T> {
    /// Spawns the http service, binding to the given address
    /// return a coroutine that you can cancel it when need to stop the service
    pub fn start<L: ToSocketAddrs>(self, addr: L) -> io::Result<coroutine::JoinHandle<()>> {
        let listener = TcpListener::bind(addr)?;
        go!(
            coroutine::Builder::new().name("TcpServer".to_owned()),
            move || {
                let server = Arc::new(self);
                let manager = Manager::new();
                for stream in listener.incoming() {
                    let mut stream = t!(stream);
                    let server = server.clone();
                    manager.add(move |_| {
                        let mut buf = BytesMut::with_capacity(512);
                        loop {
                            match request::decode(&mut buf) {
                                Ok(None) => {
                                    // need more data
                                    let mut temp_buf = [0; 512];
                                    match stream.read(&mut temp_buf) {
                                        Ok(0) => return, // connection was closed
                                        Ok(n) => {
                                            buf.reserve(n);
                                            buf.put_slice(&temp_buf[0..n]);
                                        }
                                        Err(err) => {
                                            match err.kind() {
                                                io::ErrorKind::UnexpectedEof
                                                | io::ErrorKind::ConnectionReset => {
                                                    info!("http server read req: connection closed")
                                                }
                                                _ => {
                                                    error!("http server read req: err = {:?}", err)
                                                }
                                            }
                                            return;
                                        }
                                    }
                                }
                                Ok(Some(req)) => {
                                    let ret = server.0.call(req).unwrap();

                                    let mut rsp = BytesMut::with_capacity(512);
                                    response::encode(ret, &mut rsp);

                                    // send the result back to client
                                    stream
                                        .write_all(rsp.as_ref())
                                        .unwrap_or_else(|e| error!("send rsp failed: err={:?}", e));
                                }
                                Err(ref e) => {
                                    error!("error decode req: err = {:?}", e);
                                }
                            }
                        }
                    });
                }
            }
        )
    }
}
