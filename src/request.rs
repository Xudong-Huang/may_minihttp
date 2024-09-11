use std::fmt;
use std::io::{self, BufRead, Read};
use std::mem::MaybeUninit;

pub(crate) const MAX_HEADERS: usize = 16;

use bytes::{Buf, BufMut, BytesMut};
use may::net::TcpStream;

use crate::http_server::err;

pub struct BodyReader<'buf, 'stream> {
    // remaining bytes for body
    req_buf: &'buf mut BytesMut,
    // the max body length limit
    body_limit: usize,
    // total read count
    total_read: usize,
    // used to read extra body bytes
    stream: &'stream mut TcpStream,
}

impl<'buf, 'stream> BodyReader<'buf, 'stream> {
    pub fn body_limit(&self) -> usize {
        self.body_limit
    }

    fn read_more_data(&mut self) -> io::Result<usize> {
        crate::http_server::reserve_buf(self.req_buf);
        let read_buf: &mut [u8] = unsafe { std::mem::transmute(self.req_buf.chunk_mut()) };
        let n = self.stream.read(read_buf)?;
        unsafe { self.req_buf.advance_mut(n) };
        Ok(n)
    }
}

impl<'buf, 'stream> Read for BodyReader<'buf, 'stream> {
    // the user should control the body reading, don't exceeds the body!
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.total_read >= self.body_limit {
            return Ok(0);
        }

        loop {
            if !self.req_buf.is_empty() {
                let min_len = buf.len().min(self.body_limit - self.total_read);
                let n = self.req_buf.reader().read(&mut buf[..min_len])?;
                self.total_read += n;
                println!(
                    "buf: {}, offset={}",
                    std::str::from_utf8(buf).unwrap(),
                    self.total_read
                );
                return Ok(n);
            }

            if self.read_more_data()? == 0 {
                return Ok(0);
            }
        }
    }
}

impl<'buf, 'stream> BufRead for BodyReader<'buf, 'stream> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        let remain = self.body_limit - self.total_read;
        if remain == 0 {
            return Ok(&[]);
        }
        if self.req_buf.is_empty() {
            self.read_more_data()?;
        }
        let n = self.req_buf.len().min(remain);
        Ok(&self.req_buf.chunk()[0..n])
    }

    fn consume(&mut self, amt: usize) {
        assert!(amt <= self.body_limit - self.total_read);
        assert!(amt <= self.req_buf.len());
        self.total_read += amt;
        self.req_buf.advance(amt)
    }
}

impl<'buf, 'stream> Drop for BodyReader<'buf, 'stream> {
    fn drop(&mut self) {
        // consume all the remaining bytes
        while let Ok(n) = self.fill_buf().map(|b| b.len()) {
            if n == 0 {
                break;
            }
            // println!("drop: {:?}", n);
            self.consume(n);
        }
    }
}

// we should hold the mut ref of req_buf
// before into body, this req_buf is only for holding headers
// after into body, this req_buf is mutable to read extra body bytes
// and the headers buf can be reused
pub struct Request<'buf, 'header, 'stream> {
    req: httparse::Request<'header, 'buf>,
    req_buf: &'buf mut BytesMut,
    stream: &'stream mut TcpStream,
}

impl<'buf, 'header, 'stream> Request<'buf, 'header, 'stream> {
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

    pub fn body(self) -> BodyReader<'buf, 'stream> {
        BodyReader {
            body_limit: self.content_length(),
            total_read: 0,
            stream: self.stream,
            req_buf: self.req_buf,
        }
    }

    fn content_length(&self) -> usize {
        let mut len = usize::MAX;
        for header in self.req.headers.iter() {
            if header.name.eq_ignore_ascii_case("content-length") {
                len = std::str::from_utf8(header.value).unwrap().parse().unwrap();
                break;
            }
        }
        len
    }
}

impl<'buf, 'header, 'stream> fmt::Debug for Request<'buf, 'header, 'stream> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "<HTTP Request {} {}>", self.method(), self.path())
    }
}

pub fn decode<'header, 'buf, 'stream>(
    headers: &'header mut [MaybeUninit<httparse::Header<'buf>>; MAX_HEADERS],
    req_buf: &'buf mut BytesMut,
    stream: &'stream mut TcpStream,
) -> io::Result<Option<Request<'buf, 'header, 'stream>>> {
    let mut req = httparse::Request::new(&mut []);
    // safety: don't hold the reference of req_buf
    // so we can transfer the mutable reference to Request
    let buf: &[u8] = unsafe { std::mem::transmute(req_buf.chunk()) };
    let status = match req.parse_with_uninit_headers(buf, headers) {
        Ok(s) => s,
        Err(e) => {
            let msg = format!("failed to parse http request: {e:?}");
            eprintln!("{msg}");
            return err(io::Error::new(io::ErrorKind::Other, msg));
        }
    };

    let len = match status {
        httparse::Status::Complete(amt) => amt,
        httparse::Status::Partial => return Ok(None),
    };
    req_buf.advance(len);

    // println!("req: {:?}", std::str::from_utf8(req_buf).unwrap());
    Ok(Some(Request {
        req,
        req_buf,
        stream,
    }))
}
