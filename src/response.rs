use bytes::BytesMut;

use std::io;

pub struct Response<'a> {
    headers: [&'static str; 16],
    headers_len: usize,
    // when set header would be ignored, only `Date` header would be appended
    fixed_header: &'static [u8],
    status_message: StatusMessage,
    body: Body,
    rsp_buf: &'a mut BytesMut,
}

enum Body {
    Str(&'static str),
    Vec(Vec<u8>),
    Dummy,
}

struct StatusMessage {
    code: &'static str,
    msg: &'static str,
}

impl<'a> Response<'a> {
    pub(crate) fn new(rsp_buf: &'a mut BytesMut) -> Response {
        let headers: [&'static str; 16] = [""; 16];

        Response {
            headers,
            headers_len: 0,
            fixed_header: b"",
            body: Body::Dummy,
            status_message: StatusMessage {
                code: "200",
                msg: "Ok",
            },
            rsp_buf,
        }
    }

    #[inline]
    pub fn status_code(&mut self, code: &'static str, msg: &'static str) -> &mut Self {
        self.status_message = StatusMessage { code, msg };
        self
    }

    #[inline]
    pub fn header(&mut self, header: &'static str) -> &mut Self {
        self.headers[self.headers_len] = header;
        self.headers_len += 1;
        self
    }

    /// when set static header, the `header` is not used any more
    #[inline]
    pub fn fixed_header(&mut self, header: &'static [u8]) -> &mut Self {
        self.fixed_header = header;
        self
    }

    #[inline]
    pub fn body(&mut self, s: &'static str) {
        self.body = Body::Str(s);
    }

    #[inline]
    pub fn body_vec(&mut self, v: Vec<u8>) {
        self.body = Body::Vec(v);
    }

    #[inline]
    pub fn body_mut(&mut self) -> &mut BytesMut {
        match self.body {
            Body::Dummy => {}
            Body::Str(s) => {
                self.rsp_buf.extend_from_slice(s.as_bytes());
                self.body = Body::Dummy;
            }
            Body::Vec(ref v) => {
                self.rsp_buf.extend_from_slice(v);
                self.body = Body::Dummy;
            }
        }
        self.rsp_buf
    }

    #[inline]
    fn body_len(&self) -> usize {
        match self.body {
            Body::Dummy => self.rsp_buf.len(),
            Body::Str(s) => s.len(),
            Body::Vec(ref v) => v.len(),
        }
    }

    #[inline]
    fn get_body(&mut self) -> &[u8] {
        match self.body {
            Body::Dummy => self.rsp_buf.as_ref(),
            Body::Str(s) => s.as_bytes(),
            Body::Vec(ref v) => v,
        }
    }

    #[inline]
    fn clear_body(&mut self) {
        match self.body {
            Body::Dummy => self.rsp_buf.clear(),
            Body::Str(_) => {}
            Body::Vec(_) => {}
        }
    }
}

pub fn encode(mut rsp: Response, buf: &mut BytesMut) {
    if !rsp.fixed_header.is_empty() {
        buf.extend_from_slice(rsp.fixed_header);
        buf.extend_from_slice(&crate::date::get_date_header());
        buf.extend_from_slice(rsp.get_body());
        rsp.clear_body();
        return;
    }

    if rsp.status_message.msg == "Ok" {
        buf.extend_from_slice(b"HTTP/1.1 200 Ok\r\nServer: M\r\nDate: ");
    } else {
        buf.extend_from_slice(b"HTTP/1.1 ");
        buf.extend_from_slice(rsp.status_message.code.as_bytes());
        buf.extend_from_slice(b" ");
        buf.extend_from_slice(rsp.status_message.msg.as_bytes());
        buf.extend_from_slice(b"\r\nServer: M\r\nDate: ");
    }
    crate::date::append_date(buf);
    buf.extend_from_slice(b"\r\nContent-Length: ");
    let mut length = itoa::Buffer::new();
    buf.extend_from_slice(length.format(rsp.body_len()).as_bytes());

    for i in 0..rsp.headers_len {
        let h = *unsafe { rsp.headers.get_unchecked(i) };
        buf.extend_from_slice(b"\r\n");
        buf.extend_from_slice(h.as_bytes());
    }

    buf.extend_from_slice(b"\r\n\r\n");
    buf.extend_from_slice(rsp.get_body());
    rsp.clear_body();
}

// impl io::Write for the response body
pub struct BodyWriter<'a>(pub &'a mut BytesMut);

impl<'a> io::Write for BodyWriter<'a> {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
