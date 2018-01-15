use std::io::{self, BufReader, Write};
use std::net::ToSocketAddrs;
use std::sync::Arc;

use co_managed::Manager;
use may::coroutine;
use may::net::TcpListener;
use may::sync::Mutex;

use Request;
use Response;

pub trait HttpService {
    fn call(&self, _request: Request) -> io::Result<Response>;
}

// this is a kind of server
pub struct HttpServer<T>(T);

macro_rules! t {
    ($e: expr) => (match $e {
        Ok(val) => val,
        Err(err) => {
            error!("call = {:?}\nerr = {:?}", stringify!($e), err);
            continue;
        }
    })
}

impl<T: HttpService> HttpServer<T> {
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
                    let stream = t!(stream);
                    let server = server.clone();
                    manager.add(move |_| {
                        let rs = stream.try_clone().expect("failed to clone stream");
                        // the read half of the stream
                        let mut rs = BufReader::new(rs);
                        // the write half need to be protected by mutex
                        // for that coroutine io obj can't shared safely
                        let ws = Arc::new(Mutex::new(stream));

                        loop {
                            let req = match Frame::decode_from(&mut rs) {
                                Ok(r) => r,
                                Err(ref e) => {
                                    if e.kind() == io::ErrorKind::UnexpectedEof {
                                        info!("tcp server decode req: connection closed");
                                    } else {
                                        error!("tcp server decode req: err = {:?}", e);
                                    }
                                    break;
                                }
                            };

                            info!("get request: id={:?}", req.id);
                            let w_stream = ws.clone();
                            let server = server.clone();
                            go!(move || {
                                let mut rsp = RspBuf::new();
                                let ret = server.0.call(req.decode_req(), &mut rsp);
                                let data = rsp.finish(req.id, ret);

                                info!("send rsp: id={}", req.id);
                                // send the result back to client
                                w_stream
                                    .lock()
                                    .unwrap()
                                    .write_all(&data)
                                    .unwrap_or_else(|e| error!("send rsp failed: err={:?}", e));
                            });
                        }
                    });
                }
            }
        )
    }
}
