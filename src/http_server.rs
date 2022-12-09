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
    // create a new http service for each connection
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

#[allow(dead_code)]
#[inline]
fn reserve_buf(buf: &mut BytesMut, len: usize) {
    let remaining = buf.capacity();
    if remaining < 1024 {
        buf.reserve(len - remaining);
    }
}

#[cfg(unix)]
#[inline]
fn nonblock_read(stream: &mut impl Read, req_buf: &mut BytesMut) -> io::Result<usize> {
    reserve_buf(req_buf, 4096 * 32);
    let mut read_cnt = 0;
    loop {
        let read_buf: &mut [u8] = unsafe { std::mem::transmute(&mut *req_buf.chunk_mut()) };
        match stream.read(read_buf) {
            Ok(n) => {
                if n > 0 {
                    read_cnt += n;
                    unsafe { req_buf.advance_mut(n) };
                } else {
                    //connection was closed
                    return Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed"));
                }
            }
            Err(err) => {
                if err.kind() == io::ErrorKind::WouldBlock {
                    break;
                }
                // info!("http server read req: connection closed");
                return Err(err);
            }
        }
    }
    Ok(read_cnt)
}

#[cfg(unix)]
#[inline]
fn nonblock_write(stream: &mut impl Write, write_buf: &mut BytesMut) -> io::Result<usize> {
    let len = write_buf.len();
    let mut written = 0;
    while written < len {
        match stream.write(&write_buf[written..]) {
            Ok(n) => {
                if n > 0 {
                    written += n;
                } else {
                    return Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed"));
                }
            }
            Err(err) => {
                if err.kind() == io::ErrorKind::WouldBlock {
                    break;
                }
                return Err(err);
            }
        }
    }

    write_buf.advance(written);

    Ok(written)
}

/// this is the generic type http server
/// with a type parameter that impl `HttpService` trait
///
pub struct HttpServer<T>(pub T);

#[cfg(unix)]
fn each_connection_loop<T: HttpService>(mut stream: TcpStream, mut service: T) {
    let mut req_buf = BytesMut::with_capacity(4096 * 32);
    let mut rsp_buf = BytesMut::with_capacity(4096 * 32);
    let mut body_buf = BytesMut::with_capacity(4096 * 8);

    loop {
        stream.reset_io();

        let inner_stream = stream.inner_mut();

        // read the socket for requests
        let read_cnt = match nonblock_read(inner_stream, &mut req_buf) {
            Ok(n) => n,
            Err(e) => return error!("read err = {:?}", e),
        };

        // prepare the requests
        reserve_buf(&mut rsp_buf, 4096 * 32);
        if read_cnt > 0 {
            while let Some(req) = t!(request::decode(&mut req_buf)) {
                let mut rsp = Response::new(&mut body_buf);
                match service.call(req, &mut rsp) {
                    Ok(()) => response::encode(rsp, &mut rsp_buf),
                    Err(e) => {
                        let err_rsp = internal_error_rsp(e, &mut body_buf);
                        response::encode(err_rsp, &mut rsp_buf);
                    }
                }
            }
        }

        // write out the responses
        match nonblock_write(inner_stream, &mut rsp_buf) {
            Ok(_) => stream.wait_io(),
            Err(e) => return error!("write err = {:?}", e),
        }
    }
}

#[cfg(not(unix))]
fn each_connection_loop<T: HttpService>(mut stream: TcpStream, mut service: T) {
    let mut req_buf = BytesMut::with_capacity(4096 * 32);
    let mut rsp_buf = BytesMut::with_capacity(4096 * 32);
    let mut body_buf = BytesMut::with_capacity(4096 * 32);
    loop {
        // read the socket for requests
        reserve_buf(&mut req_buf, 4096 * 32);

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

        // prepare the requests
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
