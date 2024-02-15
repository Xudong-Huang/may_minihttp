use bytes::BytesMut;

use std::mem::MaybeUninit;
use std::{fmt, io};

pub(crate) const MAX_HEADERS: usize = 16;

pub struct Request<'a, 'header> {
    body: &'a [u8],
    req: httparse::Request<'header, 'a>,
    len: usize,
}

impl<'a, 'header> Request<'a, 'header> {
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

    pub fn body(&self) -> &[u8] {
        &self.body
    }

    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.len
    }
}

impl<'a, 'header> fmt::Debug for Request<'a, 'header> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "<HTTP Request {} {}>", self.method(), self.path())
    }
}

pub fn decode<'a, 'header>(
    buf: &'a BytesMut,
    headers: &'header mut [MaybeUninit<httparse::Header<'a>>; MAX_HEADERS],
) -> io::Result<Option<Request<'a, 'header>>> {
    let mut req = httparse::Request::new(&mut []);

    let status = match req.parse_with_uninit_headers(buf, headers) {
        Ok(s) => s,
        Err(e) => {
            let msg = format!("failed to parse http request: {e:?}");
            return Err(io::Error::new(io::ErrorKind::Other, msg));
        }
    };

    let len = match status {
        httparse::Status::Complete(amt) => amt,
        httparse::Status::Partial => return Ok(None),
    };

    let body = &buf[len..];
    let len = len + body.len();
    Ok(Some(Request { req, body, len }))
}
