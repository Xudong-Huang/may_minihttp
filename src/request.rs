use bytes::BytesMut;

use std::mem::MaybeUninit;
use std::{fmt, io};

const MAX_HEADERS: usize = 16;

pub struct Request {
    req: httparse::Request<'static, 'static>,
    _headers: [MaybeUninit<httparse::Header<'static>>; MAX_HEADERS],
    _data: BytesMut,
}

impl Request {
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
}

impl fmt::Debug for Request {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "<HTTP Request {} {}>", self.method(), self.path())
    }
}

pub fn decode(buf: &mut BytesMut) -> io::Result<Option<Request>> {
    let mut headers = unsafe {
        MaybeUninit::<[MaybeUninit<httparse::Header<'_>>; MAX_HEADERS]>::uninit().assume_init()
    };

    let mut req = httparse::Request::new(&mut []);

    let status = match req.parse_with_uninit_headers(buf, &mut headers) {
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
        req: unsafe { std::mem::transmute(req) },
        _headers: unsafe { std::mem::transmute(headers) },
        _data: buf.split_to(amt),
    }))
}
