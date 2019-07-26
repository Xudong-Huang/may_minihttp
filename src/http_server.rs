//! http server implementation on top of `MAY`

use std::error::Error;
use std::io::{self, Read, Write};
use std::net::ToSocketAddrs;

use crate::request::{self, Request};
use crate::response::{self, Response};
use bytes::{BufMut, BytesMut};
use may::net::TcpListener;
use may::{coroutine, go};

macro_rules! t {
    ($e: expr) => {
        match $e {
            Ok(val) => val,
            #[cold]
            Err(err) => {
                if err.kind() == io::ErrorKind::ConnectionReset
                    || err.kind() == io::ErrorKind::UnexpectedEof
                {
                    // info!("http server read req: connection closed");
                    return;
                }

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
            #[cold]
            Err(err) => {
                error!("call = {:?}\nerr = {:?}", stringify!($e), err);
                continue;
            }
        }
    };
}

/// the http service trait
/// user code should supply a type that impl the `call` method for the http server
///
pub trait HttpService {
    fn call(&mut self, request: Request) -> io::Result<Response>;
}

pub trait HttpServiceFactory: Send + Sized + 'static {
    type Service: HttpService + Send;
    // creat a new http service for each connection
    fn new_service(&self) -> Self::Service;

    /// Spawns the http service, binding to the given address
    /// return a coroutine that you can cancel it when need to stop the service
    fn start<L: ToSocketAddrs>(self, addr: L) -> io::Result<coroutine::JoinHandle<()>> {
        let listener = TcpListener::bind(addr)?;
        go!(
            coroutine::Builder::new().name("TcpServerFac".to_owned()),
            move || {
                for stream in listener.incoming() {
                    let stream = t_c!(stream);
                    let service = self.new_service();
                    go!(move || each_connection_loop(stream, service));
                }
            }
        )
    }
}

fn internal_error_rsp(e: io::Error) -> Response {
    error!("error in service: err = {:?}", e);
    let mut err_rsp = Response::new();
    err_rsp.status_code("500", "Internal Server Error");
    err_rsp
        .body_mut()
        .extend_from_slice(e.description().as_bytes());
    err_rsp
}

/// this is the generic type http server
/// with a type parameter that impl `HttpService` trait
///
pub struct HttpServer<T>(pub T);

fn each_connection_loop<S, T>(mut stream: S, mut service: T)
where
    S: Read + Write,
    T: HttpService,
{
    let mut buf = BytesMut::with_capacity(4096 * 8);
    let mut rsps = BytesMut::with_capacity(4096 * 8);
    loop {
        // read the socket for reqs
        if buf.remaining_mut() < 1024 {
            buf.reserve(4096 * 8);
        }

        let n = {
            let read_buf = unsafe { buf.bytes_mut() };
            t!(stream.read(read_buf))
        };
        //connection was closed
        if n == 0 {
            #[cold]
            return;
        }
        unsafe { buf.advance_mut(n) };

        if buf.remaining_mut() < 1024 {
            buf.reserve(4096 * 8);
        }

        // prepare the reqs
        while let Some(req) = t!(request::decode(&mut buf)) {
            let ret = service.call(req).unwrap_or_else(internal_error_rsp);
            response::encode(ret, &mut rsps);
        }

        // send the result back to client
        t!(stream.write_all(rsps.as_ref()));
        rsps.clear();
    }
}

impl<T: HttpService + Clone + Send + Sync + 'static> HttpServer<T> {
    /// Spawns the http service, binding to the given address
    /// return a coroutine that you can cancel it when need to stop the service
    pub fn start<L: ToSocketAddrs>(self, addr: L) -> io::Result<coroutine::JoinHandle<()>> {
        let listener = TcpListener::bind(addr)?;
        let service = self.0;
        go!(
            coroutine::Builder::new().name("TcpServer".to_owned()),
            move || {
                for stream in listener.incoming() {
                    let stream = t_c!(stream);
                    let service = service.clone();
                    go!(move || each_connection_loop(stream, service));
                }
            }
        )
    }
}
