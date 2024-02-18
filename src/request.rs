use bytes::BytesMut;
use may::net::TcpStream;

use std::fmt;
use std::io::{self, Read};
use std::mem::MaybeUninit;

pub(crate) const MAX_HEADERS: usize = 16;

pub struct Request<'a, 'header, 'stream, 'offset> {
    req: httparse::Request<'header, 'a>,
    len: usize,
    // remaining bytes for body
    body_buf: &'a [u8],
    // track how many bytes have been read from the body
    body_offset: &'offset mut usize,
    // used to read extra body bytes
    stream: &'stream mut TcpStream,
}

impl<'a, 'header, 'stream, 'offset> Request<'a, 'header, 'stream, 'offset> {
    pub fn method(&self) -> &str {
        self.req.method.unwrap()
    }

    pub fn path(&self) -> &str {
        self.req.path.unwrap()
    }

    pub fn version(&self) -> u8 {
        self.req.version.unwrap()
    }

    pub fn headers(&self) -> &[httparse::Header<'_>] {
        self.req.headers
    }

    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.len
    }
}

impl<'a, 'header, 'stream, 'offset> Read for Request<'a, 'header, 'stream, 'offset> {
    // the user should control the body reading, don't exceeds the body!
    // FIXME: deal with partial body
    // FIXME: deal with next request header
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let body_offset = *self.body_offset;
        let remaining = self.body_buf.len() - body_offset;
        if remaining > 0 {
            let n = buf.len().min(remaining);
            unsafe {
                buf.as_mut_ptr()
                    .copy_from_nonoverlapping(self.body_buf.as_ptr().add(body_offset), n)
            }
            *self.body_offset = body_offset + n;
            // println!(
            //     "buf: {}, offset={}",
            //     std::str::from_utf8(buf).unwrap(),
            //     body_offset + n
            // );
            Ok(n)
        } else {
            // perform nonblock_read
            match self.stream.inner_mut().read(buf) {
                Ok(n) => Ok(n),
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => Ok(0),
                Err(e) => Err(e),
            }
        }
    }
}

impl<'a, 'header, 'stream, 'offset> fmt::Debug for Request<'a, 'header, 'stream, 'offset> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "<HTTP Request {} {}>", self.method(), self.path())
    }
}

pub fn decode<'a, 'header, 'stream, 'offset>(
    buf: &'a BytesMut,
    headers: &'header mut [MaybeUninit<httparse::Header<'a>>; MAX_HEADERS],
    stream: &'stream mut TcpStream,
    body_offset: &'offset mut usize,
) -> io::Result<Option<Request<'a, 'header, 'stream, 'offset>>> {
    let mut req = httparse::Request::new(&mut []);
    *body_offset = 0;

    let status = match req.parse_with_uninit_headers(buf, headers) {
        Ok(s) => s,
        Err(e) => {
            println!("failed to parse http request: {e:?}");
            let msg = format!("failed to parse http request: {e:?}");
            return Err(io::Error::new(io::ErrorKind::Other, msg));
        }
    };

    let len = match status {
        httparse::Status::Complete(amt) => amt,
        httparse::Status::Partial => return Ok(None),
    };

    Ok(Some(Request {
        req,
        len,
        body_buf: &buf[len..],
        stream,
        body_offset,
    }))
}
