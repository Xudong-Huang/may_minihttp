use bytes::{BufMut, BytesMut};
use may::net::TcpStream;

use std::io::Read;
use std::{fmt, io};

pub struct Request<'headers, 'req, 'stream> {
    pub parameters: httparse::Request<'headers, 'req>,
    data: &'req [u8],
    pub body: Body<'req, 'stream>,
}

pub struct Body<'req, 'stream> {
    buf: &'req [u8],
    stream: &'stream mut TcpStream,
    wrote_body: usize,
}

impl<'req, 'stream> Read for Body<'req, 'stream> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.wrote_body == self.buf.len() {
            match self.stream.read(buf) {
                Ok(n) => Ok(n),
                Err(err) => {
                    if err.kind() == io::ErrorKind::WouldBlock {
                        Ok(0)
                    } else {
                        Err(err)
                    }
                }
            }
        } else {
            match self.buf.read(buf) {
                Ok(n) => {
                    self.wrote_body += n;
                    Ok(n)
                }
                err @ Err(_) => err,
            }
        }
    }
}

impl<'req, 'stream> Body<'req, 'stream> {
    /// This is preferable over using `std::io::Read` if your `Body` is small.
    pub fn resolve(self) -> BytesMut {
        let mut req_buf = BytesMut::with_capacity(4096 * 8);
        req_buf.extend_from_slice(self.buf);
        loop {
            // read the socket for requests
            let remaining = req_buf.capacity() - req_buf.len();
            if remaining < 512 {
                req_buf.reserve(4096 * 8 - remaining);
            }

            let buf = req_buf.chunk_mut();
            let read_buf = unsafe { std::slice::from_raw_parts_mut(buf.as_mut_ptr(), buf.len()) };
            match self.stream.read(read_buf) {
                Ok(n) => {
                    if n == 0 {
                        //connection was closed
                        return req_buf;
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
                        return req_buf;
                    }
                    error!("call = {:?}\nerr = {:?}", stringify!($e), err);
                    return req_buf;
                }
            }
        }
        return req_buf;
    }
}

impl<'headers, 'req, 'stream> Request<'headers, 'req, 'stream> {
    pub fn headers(&self) -> &[httparse::Header] {
        &*self.parameters.headers
    }
}

impl<'headers, 'req, 'stream> fmt::Debug for Request<'headers, 'req, 'stream> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "<HTTP Request {:?} {:?}>",
            self.parameters.method, self.parameters.path
        )
    }
}

pub fn decode<'headers, 'req, 'stream>(
    buf: &'req BytesMut,
    headers: &'headers mut [httparse::Header<'req>; 16],
    stream: &'stream mut TcpStream,
) -> io::Result<Option<Request<'headers, 'req, 'stream>>> {
    unsafe {
        println!("{:?}", std::str::from_utf8_unchecked(&buf));
    }
    let mut r = httparse::Request::new(headers);

    let status = match r.parse(buf) {
        Ok(s) => s,
        Err(e) => {
            let msg = format!("failed to parse http request: {:?}", e);
            return Err(io::Error::new(io::ErrorKind::Other, msg));
        }
    };

    let amt = match status {
        httparse::Status::Complete(amt) => amt,
        httparse::Status::Partial => return Ok(None),
    };

    Ok(Some(Request {
        parameters: r,
        data: &buf[0..amt],
        body: {
            Body {
                buf: &buf[amt..],
                stream,
                wrote_body: 0,
            }
        },
    }))
}
