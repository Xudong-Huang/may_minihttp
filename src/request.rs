use std::{fmt, io, slice, str};

use bytes::BytesMut;

pub struct Request {
    method: Slice,
    path: Slice,
    version: u8,
    headers: [(Slice, Slice); 16],
    headers_len: usize,
    data: BytesMut,
}

type Slice = (usize, usize);

pub struct RequestHeaders<'req> {
    headers: slice::Iter<'req, (Slice, Slice)>,
    req: &'req Request,
}

impl Request {
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

    pub fn body(&self) -> &[u8] {
        unimplemented!()
    }

    fn slice(&self, slice: &Slice) -> &[u8] {
        &self.data[slice.0..slice.1]
    }
}

impl fmt::Debug for Request {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "<HTTP Request {} {}>", self.method(), self.path())
    }
}

pub fn decode(buf: &mut BytesMut) -> io::Result<Option<Request>> {
    let mut headers: [httparse::Header; 16] =
        unsafe { std::mem::MaybeUninit::uninit().assume_init() };
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

    let mut headers: [(Slice, Slice); 16] =
        unsafe { std::mem::MaybeUninit::uninit().assume_init() };
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
