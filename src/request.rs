use bytes::BytesMut;

use std::mem::MaybeUninit;
use std::{fmt, io};

const MAX_HEADERS: usize = 16;

pub struct Request<'a> {
    req: httparse::Request<'a, 'a>,
    _headers: [MaybeUninit<httparse::Header<'a>>; MAX_HEADERS],
    data: &'a [u8],
}

impl<'a> Request<'a> {
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
        unimplemented!()
    }

    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.data.len()
    }
}

impl<'a> fmt::Debug for Request<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "<HTTP Request {} {}>", self.method(), self.path())
    }
}

pub fn decode(buf: &BytesMut) -> io::Result<Option<Request>> {
    let mut headers = [MaybeUninit::<httparse::Header<'_>>::uninit(); MAX_HEADERS];
    let mut req = httparse::Request::new(&mut []);

    let status = match req.parse_with_uninit_headers(buf, &mut headers) {
        Ok(s) => s,
        Err(e) => {
            let msg = format!("failed to parse http request: {e:?}");
            return Err(io::Error::new(io::ErrorKind::Other, msg));
        }
    };

    let amt = match status {
        httparse::Status::Complete(amt) => amt,
        httparse::Status::Partial => return Ok(None),
    };

    Ok(Some(Request {
        req: unsafe { std::mem::transmute(req) },
        _headers: headers,
        data: &buf[..amt],
    }))
}
