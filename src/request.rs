use bytes::buf::Reader;
use bytes::{Buf, BufMut, BytesMut};
use may::net::TcpStream;

use std::io::Read;
use std::mem::MaybeUninit;
use std::{fmt, io, slice, str};

pub struct Request<'req> {
    method: Slice,
    path: Slice,
    version: u8,
    headers: [(Slice, Slice); 16],
    headers_len: usize,
    data: BytesMut,
    pub body: Body<'req>,
}

pub struct Body<'req> {
    buf: Reader<BytesMut>,
    buf_len: usize,
    stream: &'req mut TcpStream,
    wrote_body: usize,
}

impl<'req> Read for Body<'req> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.wrote_body == self.buf_len {
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

impl<'req> Body<'req> {
    /// This is preferable over using `std::io::Read` if your `Body` is small.
    pub fn resolve(self) -> BytesMut {
        let mut req_buf = self.buf.into_inner();
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

type Slice = (usize, usize);

pub struct RequestHeaders<'req> {
    headers: slice::Iter<'req, (Slice, Slice)>,
    req: &'req Request<'req>,
}

impl<'req> Request<'req> {
    pub fn method(&self) -> &str {
        // str::from_utf8(self.slice(&self.method)).unwrap()
        unsafe { str::from_utf8_unchecked(self.slice(&self.method)) }
    }

    pub fn path(&self) -> &str {
        // str::from_utf8(self.slice(&self.path)).unwrap()
        unsafe { str::from_utf8_unchecked(self.slice(&self.path)) }
    }

    pub fn version(&self) -> u8 {
        self.version
    }

    pub fn headers(&self) -> RequestHeaders {
        RequestHeaders {
            headers: self.headers[..self.headers_len].iter(),
            req: self,
        }
    }

    fn slice(&self, slice: &Slice) -> &[u8] {
        &self.data[slice.0..slice.1]
    }
}

impl<'req> fmt::Debug for Request<'req> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "<HTTP Request {} {}>", self.method(), self.path())
    }
}

pub fn decode<'req>(
    buf: &mut BytesMut,
    stream: &'req mut TcpStream,
) -> io::Result<Option<Request<'req>>> {
    let mut headers: [httparse::Header; 16] = unsafe {
        let h: [MaybeUninit<httparse::Header>; 16] = MaybeUninit::uninit().assume_init();
        std::mem::transmute(h)
    };
    let mut r = httparse::Request::new(&mut headers);

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

    let toslice = |a: &[u8]| {
        let start = a.as_ptr() as usize - buf.as_ptr() as usize;
        debug_assert!(start < buf.len());
        (start, start + a.len())
    };

    let mut headers: [(Slice, Slice); 16] = unsafe {
        let h: [MaybeUninit<(Slice, Slice)>; 16] = MaybeUninit::uninit().assume_init();
        std::mem::transmute(h)
    };
    let mut headers_len = 0;
    for h in r.headers.iter() {
        debug_assert!(headers_len < 16);
        *unsafe { headers.get_unchecked_mut(headers_len) } =
            (toslice(h.name.as_bytes()), toslice(h.value));
        headers_len += 1;
    }

    Ok(Some(Request {
        method: toslice(r.method.unwrap().as_bytes()),
        path: toslice(r.path.unwrap().as_bytes()),
        version: r.version.unwrap(),
        headers,
        headers_len,
        data: buf.split_to(amt),
        body: {
            let sp = buf.split_to(buf.len());

            Body {
                buf_len: sp.len(),
                buf: sp.reader(),
                stream,
                wrote_body: 0,
            }
        },
    }))
}

impl<'req> Iterator for RequestHeaders<'req> {
    type Item = (&'req str, &'req [u8]);

    fn next(&mut self) -> Option<(&'req str, &'req [u8])> {
        self.headers.next().map(|&(ref a, ref b)| {
            let a = self.req.slice(a);
            let b = self.req.slice(b);
            (unsafe { str::from_utf8_unchecked(a) }, b)
        })
    }
}
