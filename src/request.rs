use bytes::BytesMut;
use may::net::TcpStream;

use std::fmt;
use std::io::{self, Read};
use std::mem::MaybeUninit;

pub(crate) const MAX_HEADERS: usize = 16;

pub struct BodyReader<'a, 'stream> {
    // remaining bytes for body
    body_buf: &'a [u8],
    // track how many bytes have been read from the body
    body_offset: usize,
    // the max body length limit
    body_limit: usize,
    // total read count
    total_read: usize,
    // used to read extra body bytes
    stream: &'stream mut TcpStream,
}

impl<'a, 'stream> BodyReader<'a, 'stream> {
    pub(crate) fn new(stream: &'stream mut TcpStream) -> Self {
        BodyReader {
            body_buf: &[],
            body_offset: 0,
            body_limit: usize::MAX,
            total_read: 0,
            stream,
        }
    }

    pub(crate) fn body_offset(&self) -> usize {
        self.body_offset
    }

    fn set_body_buf(&mut self, body_buf: &'a [u8]) {
        self.body_buf = body_buf;
        self.body_offset = 0;
    }

    fn set_body_limit(&mut self, body_limit: usize) {
        self.body_limit = body_limit;
    }
}

pub struct Request<'a, 'header, 'stream, 'body> {
    req: httparse::Request<'header, 'a>,
    len: usize,
    body: &'body mut BodyReader<'a, 'stream>,
}

impl<'a, 'header, 'stream, 'body> Request<'a, 'header, 'stream, 'body> {
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

    pub fn body(&mut self) -> &mut BodyReader<'a, 'stream> {
        self.body.set_body_limit(self.content_length());
        self.body
    }

    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    fn content_length(&self) -> usize {
        let mut len = usize::MAX;
        for header in self.req.headers.iter() {
            if header.name.eq_ignore_ascii_case("content-length") {
                len = std::str::from_utf8(header.value)
                    .unwrap()
                    .parse()
                    .unwrap_or(usize::MAX);
                break;
            }
        }
        len
    }
}

impl<'a, 'stream> Read for BodyReader<'a, 'stream> {
    // the user should control the body reading, don't exceeds the body!
    // FIXME: deal with partial body
    // FIXME: deal with next request header
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.total_read >= self.body_limit {
            return Ok(0);
        }

        let body_offset = self.body_offset;
        let remaining = self.body_buf.len() - body_offset;
        if remaining > 0 {
            let n = buf.len().min(remaining);
            unsafe {
                buf.as_mut_ptr()
                    .copy_from_nonoverlapping(self.body_buf.as_ptr().add(body_offset), n)
            }
            self.total_read += n;
            self.body_offset = body_offset + n;
            // println!(
            //     "buf: {}, offset={}",
            //     std::str::from_utf8(buf).unwrap(),
            //     body_offset + n
            // );
            Ok(n)
        } else {
            // perform nonblock_read
            match self.stream.inner_mut().read(buf) {
                Ok(n) => {
                    self.total_read += n;
                    Ok(n)
                }
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => Ok(0),
                Err(e) => Err(e),
            }
        }
    }
}

impl<'a, 'header, 'stream, 'body> fmt::Debug for Request<'a, 'header, 'stream, 'body> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "<HTTP Request {} {}>", self.method(), self.path())
    }
}

pub fn decode<'a, 'header, 'stream, 'body>(
    buf: &'a BytesMut,
    body: &'body mut BodyReader<'a, 'stream>,
    headers: &'header mut [MaybeUninit<httparse::Header<'a>>; MAX_HEADERS],
) -> io::Result<Option<Request<'a, 'header, 'stream, 'body>>> {
    let mut req = httparse::Request::new(&mut []);

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
    println!("req: {:?}", std::str::from_utf8(buf).unwrap());

    body.set_body_buf(&buf[len..]);

    Ok(Some(Request { req, len, body }))
}
