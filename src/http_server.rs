//! http server implementation on top of `MAY`

use std::collections::VecDeque;
use std::error::Error;
use std::io::{self, Read, Write};
use std::net::ToSocketAddrs;
use std::sync::Arc;

use bytes::{BufMut, BytesMut};
use may::coroutine;
use may::net::TcpListener;
use request::{self, Request};
use response::{self, Response};

/// the http service trait
/// user code should supply a type that impl the `call` method for the http server
///
pub trait HttpService {
    fn call(&self, _request: Request) -> io::Result<Response>;
}

macro_rules! t {
    ($e: expr) => {
        match $e {
            Ok(val) => val,
            Err(ref err)
                if err.kind() == io::ErrorKind::ConnectionReset
                    || err.kind() == io::ErrorKind::UnexpectedEof =>
            {
                // info!("http server read req: connection closed");
                return;
            }
            Err(err) => {
                error!("call = {:?}\nerr = {:?}", stringify!($e), err);
                return;
            }
        }
    };
}

macro_rules! t_c {
    ($e: expr) => {
        match $e {
            Ok(val) => val,
            Err(err) => {
                error!("call = {:?}\nerr = {:?}", stringify!($e), err);
                continue;
            }
        }
    };
}

fn internal_error_rsp(e: io::Error) -> Response {
    error!("error in service: err = {:?}", e);
    let mut err_rsp = Response::new();
    err_rsp.status_code(500, "Internal Server Error");
    err_rsp.body(e.description());
    err_rsp
}

/// this is the generic type http server
/// with a type parameter that impl `HttpService` trait
///
pub struct HttpServer<T>(pub T);

impl<T: HttpService + Send + Sync + 'static> HttpServer<T> {
    /// Spawns the http service, binding to the given address
    /// return a coroutine that you can cancel it when need to stop the service
    pub fn start<L: ToSocketAddrs>(self, addr: L) -> io::Result<coroutine::JoinHandle<()>> {
        let listener = TcpListener::bind(addr)?;
        go!(
            coroutine::Builder::new().name("TcpServer".to_owned()),
            move || {
                let server = Arc::new(self);
                for stream in listener.incoming() {
                    let mut stream = t_c!(stream);
                    let server = server.clone();
                    go!(move || {
                        let mut buf = BytesMut::with_capacity(4096);
                        let mut reqs = VecDeque::with_capacity(32);
                        let mut rsps = BytesMut::with_capacity(4096);
                        loop {
                            // read the socket for reqs
                            if buf.remaining_mut() < 1024 {
                                buf.reserve(4096);
                            }
                            let n = {
                                let read_buf = unsafe { buf.bytes_mut() };
                                t!(stream.read(read_buf))
                            };
                            //connection was closed
                            if n == 0 {
                                return;
                            }
                            unsafe { buf.advance_mut(n) };

                            // prepare the reqs
                            while let Some(req) = t!(request::decode(&mut buf)) {
                                reqs.push_back(req);
                            }

                            // process the reqs
                            while let Some(req) = reqs.pop_front() {
                                let ret = server.0.call(req).unwrap_or_else(internal_error_rsp);
                                response::encode(ret, &mut rsps);
                            }

                            // send the result back to client
                            t!(stream.write_all(rsps.as_ref()));
                            rsps.clear();
                        }
                    });
                }
            }
        )
    }
}
