//! http server implementation on top of `MAY`

use std::io::{self, Read, Write};
use std::net::ToSocketAddrs;

use crate::request::{self, Request};
use crate::response::{self, Response};
#[cfg(unix)]
use bytes::Buf;
use bytes::{BufMut, BytesMut};
#[cfg(unix)]
use may::io::WaitIo;
use may::net::{TcpListener, TcpStream};
use may::{coroutine, go};

macro_rules! t {
    ($e: expr) => {
        match $e {
            Ok(val) => val,
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
    fn call(&mut self, req: Request, rsp: &mut Response) -> io::Result<()>;
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

fn internal_error_rsp(e: io::Error, buf: &mut BytesMut) -> Response {
    error!("error in service: err = {:?}", e);
    buf.clear();
    let mut err_rsp = Response::new(buf);
    err_rsp.status_code("500", "Internal Server Error");
    err_rsp
        .body_mut()
        .extend_from_slice(e.to_string().as_bytes());
    err_rsp
}

/// this is the generic type http server
/// with a type parameter that impl `HttpService` trait
///
pub struct HttpServer<T>(pub T);

#[cfg(unix)]
fn each_connection_loop<T: HttpService>(mut stream: TcpStream, mut service: T) {
    let mut req_buf = BytesMut::with_capacity(4096 * 8);
    let mut rsp_buf = BytesMut::with_capacity(4096 * 32);
    let mut body_buf = BytesMut::with_capacity(4096 * 8);
    stream.set_nonblocking(true).unwrap();
    loop {
        stream.reset_io();
        loop {
            // read the socket for reqs
            let remaining = req_buf.capacity() - req_buf.len();
            if remaining < 512 {
                req_buf.reserve(4096 * 8 - remaining);
            }

            let buf = req_buf.chunk_mut();
            let read_buf = unsafe { std::slice::from_raw_parts_mut(buf.as_mut_ptr(), buf.len()) };
            match stream.read(read_buf) {
                Ok(n) => {
                    if n == 0 {
                        //connection was closed
                        return;
                    } else {
                        unsafe { req_buf.advance_mut(n) };
                    }
                }
                Err(err) => {
                    if err.kind() == io::ErrorKind::WouldBlock {
                        break;
                    } else if err.kind() == io::ErrorKind::ConnectionReset
                        || err.kind() == io::ErrorKind::UnexpectedEof
                    {
                        // info!("http server read req: connection closed");
                        return;
                    }
                    error!("call = {:?}\nerr = {:?}", stringify!($e), err);
                    return;
                }
            }
        }

        let remaining = rsp_buf.capacity() - rsp_buf.len();
        if remaining < 512 {
            rsp_buf.reserve(4096 * 32 - remaining);
        }

        // prepare the reqs
        while let Some(req) = t!(request::decode(&mut req_buf)) {
            let mut rsp = Response::new(&mut body_buf);
            if let Err(e) = service.call(req, &mut rsp) {
                let err_rsp = internal_error_rsp(e, &mut body_buf);
                response::encode(err_rsp, &mut rsp_buf);
            } else {
                response::encode(rsp, &mut rsp_buf);
            }
        }

        if !rsp_buf.is_empty() {
            let len = rsp_buf.len();
            let mut written = 0;
            while written < len {
                match stream.write(&rsp_buf[written..]) {
                    Ok(n) => {
                        if n == 0 {
                            return;
                        } else {
                            written += n;
                        }
                    }
                    Err(err) => {
                        if err.kind() == io::ErrorKind::WouldBlock {
                            break;
                        } else if err.kind() == io::ErrorKind::ConnectionReset
                            || err.kind() == io::ErrorKind::UnexpectedEof
                        {
                            // info!("http server read req: connection closed");
                            return;
                        }
                        error!("call = {:?}\nerr = {:?}", stringify!($e), err);
                        return;
                    }
                }
            }
            if written == len {
                unsafe { rsp_buf.set_len(0) }
            } else if written > 0 {
                rsp_buf.advance(written);
            }
        }

        stream.wait_io();
    }
}

#[cfg(not(unix))]
fn each_connection_loop<T: HttpService>(mut stream: TcpStream, mut service: T) {
    let mut req_buf = BytesMut::with_capacity(4096 * 8);
    let mut rsp_buf = BytesMut::with_capacity(4096 * 32);
    let mut body_buf = BytesMut::with_capacity(4096 * 8);
    loop {
        // read the socket for reqs
        let remaining = req_buf.capacity() - req_buf.len();
        if remaining < 512 {
            req_buf.reserve(4096 * 8 - remaining);
        }

        let n = {
            let buf = req_buf.chunk_mut();
            let read_buf = unsafe { std::slice::from_raw_parts_mut(buf.as_mut_ptr(), buf.len()) };
            t!(stream.read(read_buf))
        };
        //connection was closed
        if n == 0 {
            return;
        }
        unsafe { req_buf.advance_mut(n) };

        // prepare the reqs
        while let Some(req) = t!(request::decode(&mut req_buf)) {
            let mut rsp = Response::new(&mut body_buf);
            if let Err(e) = service.call(req, &mut rsp) {
                let err_rsp = internal_error_rsp(e, &mut body_buf);
                response::encode(err_rsp, &mut rsp_buf);
            } else {
                response::encode(rsp, &mut rsp_buf);
            }
        }

        // send the result back to client
        t!(stream.write_all(rsp_buf.as_ref()));
        rsp_buf.clear();
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
