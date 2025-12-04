//! http server implementation on top of `MAY`

use std::io::{self, Read, Write};
use std::mem::MaybeUninit;
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

/// Check if an error is a normal client disconnect (not worth logging as ERROR)
#[inline]
fn is_client_disconnect(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::BrokenPipe
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::NotConnected
            | io::ErrorKind::UnexpectedEof
    )
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
    fn new_service(&self, id: usize) -> Self::Service;

    /// Spawns the http service, binding to the given address
    /// return a coroutine that you can cancel it when need to stop the service
    fn start<L: ToSocketAddrs>(self, addr: L) -> io::Result<coroutine::JoinHandle<()>> {
        let listener = TcpListener::bind(addr)?;
        go!(
            coroutine::Builder::new().name("TcpServerFac".to_owned()),
            move || {
                #[cfg(unix)]
                use std::os::fd::AsRawFd;
                #[cfg(windows)]
                use std::os::windows::io::AsRawSocket;
                for stream in listener.incoming() {
                    let mut stream = t_c!(stream);
                    #[cfg(unix)]
                    let id = stream.as_raw_fd() as usize;
                    #[cfg(windows)]
                    let id = stream.as_raw_socket() as usize;
                    // t_c!(stream.set_nodelay(true));
                    let service = self.new_service(id);
                    let builder = may::coroutine::Builder::new().id(id);
                    go!(
                        builder,
                        move || if let Err(e) = each_connection_loop(&mut stream, service) {
                            // Only log actual errors, not normal client disconnects
                            if !is_client_disconnect(&e) {
                                error!("service err = {e:?}");
                            }
                            stream.shutdown(std::net::Shutdown::Both).ok();
                        }
                    )
                    .unwrap();
                }
            }
        )
    }
}

#[inline]
#[cold]
pub(crate) fn err<T>(e: io::Error) -> io::Result<T> {
    Err(e)
}

#[cfg(unix)]
#[inline]
fn nonblock_read(stream: &mut impl Read, req_buf: &mut BytesMut) -> io::Result<bool> {
    reserve_buf(req_buf);
    let read_buf: &mut [u8] = unsafe { std::mem::transmute(req_buf.chunk_mut()) };
    let len = read_buf.len();

    let mut read_cnt = 0;
    while read_cnt < len {
        match stream.read(unsafe { read_buf.get_unchecked_mut(read_cnt..) }) {
            Ok(0) => return err(io::Error::new(io::ErrorKind::BrokenPipe, "read closed")),
            Ok(n) => read_cnt += n,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
            Err(e) => return err(e),
        }
    }

    unsafe { req_buf.advance_mut(read_cnt) };
    Ok(read_cnt < len)
}

#[cfg(unix)]
#[inline]
fn nonblock_write(stream: &mut impl Write, rsp_buf: &mut BytesMut) -> io::Result<usize> {
    let write_buf = rsp_buf.chunk();
    let len = write_buf.len();
    let mut write_cnt = 0;
    while write_cnt < len {
        match stream.write(unsafe { write_buf.get_unchecked(write_cnt..) }) {
            Ok(0) => return err(io::Error::new(io::ErrorKind::BrokenPipe, "write closed")),
            Ok(n) => write_cnt += n,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
            Err(e) => return err(e),
        }
    }
    rsp_buf.advance(write_cnt);
    Ok(write_cnt)
}

const BUF_LEN: usize = 4096 * 8;
#[inline]
pub(crate) fn reserve_buf(buf: &mut BytesMut) {
    let rem = buf.capacity() - buf.len();
    if rem < 1024 {
        buf.reserve(BUF_LEN - rem);
    }
}

/// this is the generic type http server
/// with a type parameter that impl `HttpService` trait
///
pub struct HttpServer<T>(pub T);

/// HTTP server with configurable max headers (const generic)
///
/// Use this when you need to handle more than 16 headers.
/// Common sizes: 32 (Standard), 64 (Large), 128 (`XLarge`)
///
/// # Example
/// ```ignore
/// use may_minihttp::HttpServerWithHeaders;
/// let server = HttpServerWithHeaders::<_, 32>(my_service);
/// ```
pub struct HttpServerWithHeaders<T, const N: usize>(pub T);

#[cfg(unix)]
fn each_connection_loop<T: HttpService>(stream: &mut TcpStream, service: T) -> io::Result<()> {
    each_connection_loop_with_headers::<T, { request::MAX_HEADERS }>(stream, service)
}

#[cfg(unix)]
fn each_connection_loop_with_headers<T: HttpService, const N: usize>(
    stream: &mut TcpStream,
    mut service: T,
) -> io::Result<()> {
    let mut req_buf = BytesMut::with_capacity(BUF_LEN);
    let mut rsp_buf = BytesMut::with_capacity(BUF_LEN);
    let mut body_buf = BytesMut::with_capacity(4096);

    loop {
        let read_blocked = nonblock_read(stream.inner_mut(), &mut req_buf)?;

        // prepare the requests, we should make sure the request is fully read
        loop {
            let mut headers = [MaybeUninit::uninit(); N];
            let req = match request::decode(&mut headers, &mut req_buf, stream)? {
                Some(req) => req,
                None => break,
            };
            reserve_buf(&mut rsp_buf);
            let mut rsp = Response::new(&mut body_buf);
            match service.call(req, &mut rsp) {
                Ok(()) => response::encode(rsp, &mut rsp_buf),
                Err(e) => {
                    eprintln!("service err = {e:?}");
                    response::encode_error(e, &mut rsp_buf);
                }
            }
            // here need to use no_delay tcp option
            // nonblock_write(stream.inner_mut(), &mut rsp_buf)?;
        }

        // write out the responses
        nonblock_write(stream.inner_mut(), &mut rsp_buf)?;

        if read_blocked {
            stream.wait_io();
        }
    }
}

#[cfg(not(unix))]
fn each_connection_loop<T: HttpService>(stream: &mut TcpStream, service: T) -> io::Result<()> {
    each_connection_loop_with_headers::<T, { request::MAX_HEADERS }>(stream, service)
}

#[cfg(not(unix))]
fn each_connection_loop_with_headers<T: HttpService, const N: usize>(
    stream: &mut TcpStream,
    mut service: T,
) -> io::Result<()> {
    let mut req_buf = BytesMut::with_capacity(BUF_LEN);
    let mut rsp_buf = BytesMut::with_capacity(BUF_LEN);
    let mut body_buf = BytesMut::with_capacity(BUF_LEN);
    loop {
        // read the socket for requests
        reserve_buf(&mut req_buf);
        let read_buf: &mut [u8] = unsafe { std::mem::transmute(&mut *req_buf.chunk_mut()) };
        let read_cnt = stream.read(read_buf)?;
        if read_cnt == 0 {
            //connection was closed
            return err(io::Error::new(io::ErrorKind::BrokenPipe, "closed"));
        }
        unsafe { req_buf.advance_mut(read_cnt) };

        // prepare the requests
        if read_cnt > 0 {
            loop {
                let mut headers = [MaybeUninit::uninit(); N];
                let req = match request::decode(&mut headers, &mut req_buf, stream)? {
                    Some(req) => req,
                    None => break,
                };
                let mut rsp = Response::new(&mut body_buf);
                match service.call(req, &mut rsp) {
                    Ok(()) => response::encode(rsp, &mut rsp_buf),
                    Err(e) => {
                        eprintln!("service err = {:?}", e);
                        response::encode_error(e, &mut rsp_buf);
                    }
                }
            }
        }

        // send the result back to client
        stream.write_all(&rsp_buf)?;
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
                    let mut stream = t_c!(stream);
                    // t_c!(stream.set_nodelay(true));
                    let service = service.clone();
                    go!(
                        move || if let Err(e) = each_connection_loop(&mut stream, service) {
                            // Only log actual errors, not normal client disconnects
                            if !is_client_disconnect(&e) {
                                error!("service err = {e:?}");
                            }
                            stream.shutdown(std::net::Shutdown::Both).ok();
                        }
                    );
                }
            }
        )
    }
}

impl<T: HttpService + Clone + Send + Sync + 'static, const N: usize> HttpServerWithHeaders<T, N> {
    /// Spawns the http service with custom max headers, binding to the given address
    /// return a coroutine that you can cancel it when need to stop the service
    pub fn start<L: ToSocketAddrs>(self, addr: L) -> io::Result<coroutine::JoinHandle<()>> {
        let listener = TcpListener::bind(addr)?;
        let service = self.0;
        go!(
            coroutine::Builder::new().name("TcpServer".to_owned()),
            move || {
                for stream in listener.incoming() {
                    let mut stream = t_c!(stream);
                    // t_c!(stream.set_nodelay(true));
                    let service = service.clone();
                    go!(move || if let Err(e) =
                        each_connection_loop_with_headers::<T, N>(&mut stream, service)
                    {
                        // Only log actual errors, not normal client disconnects
                        if !is_client_disconnect(&e) {
                            error!("service err = {e:?}");
                        }
                        stream.shutdown(std::net::Shutdown::Both).ok();
                    });
                }
            }
        )
    }
}
