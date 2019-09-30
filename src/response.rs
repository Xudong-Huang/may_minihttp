use std::io;

use bytes::{BufMut, Bytes, BytesMut};

pub struct Response<'a> {
    headers: [&'static str; 16],
    headers_len: usize,
    status_message: StatusMessage,
    body: Body,
    rsp_buf: &'a mut BytesMut,
}

enum Body {
    SMsg(&'static str),
    DMsg,
}

struct StatusMessage {
    code: &'static str,
    msg: &'static str,
}

impl<'a> Response<'a> {
    pub(crate) fn new(rsp_buf: &'a mut BytesMut) -> Response {
        Response {
            headers: unsafe { std::mem::MaybeUninit::uninit().assume_init() },
            headers_len: 0,
            body: Body::DMsg,
            status_message: StatusMessage {
                code: "200",
                msg: "Ok",
            },
            rsp_buf,
        }
    }

    pub fn status_code(&mut self, code: &'static str, msg: &'static str) -> &mut Self {
        self.status_message = StatusMessage { code, msg };
        self
    }

    pub fn header(&mut self, header: &'static str) -> &mut Self {
        debug_assert!(self.headers_len < 16);
        *unsafe { self.headers.get_unchecked_mut(self.headers_len) } = header;
        self.headers_len += 1;
        self
    }

    pub fn body(&mut self, s: &'static str) {
        self.body = Body::SMsg(s);
    }

    pub fn body_mut(&mut self) -> &mut BytesMut {
        match self.body {
            Body::DMsg => {}
            Body::SMsg(s) => {
                self.rsp_buf.put_slice(s.as_bytes());
                self.body = Body::DMsg;
            }
        }
        self.rsp_buf
    }

    fn get_body(&mut self) -> Bytes {
        match self.body {
            Body::DMsg => self.rsp_buf.take().freeze(),
            Body::SMsg(s) => Bytes::from_static(s.as_bytes()),
        }
    }
}

pub fn encode(mut msg: Response, mut buf: &mut BytesMut) {
    let body = msg.get_body();
    buf.put_slice(b"HTTP/1.1 ");
    buf.put_slice(msg.status_message.code.as_bytes());
    buf.put_slice(b" ");
    buf.put_slice(msg.status_message.msg.as_bytes());
    buf.put_slice(b"\r\nServer: may\r\nDate: ");
    crate::date::now().put_bytes(buf);
    buf.put_slice(b"\r\nContent-Length: ");
    itoa::fmt(&mut buf, body.len()).unwrap();
    buf.put_slice(b"\r\n");

    for i in 0..msg.headers_len {
        let h = *unsafe { msg.headers.get_unchecked(i) };
        buf.put_slice(h.as_bytes());
        buf.put_slice(b"\r\n");
    }

    buf.put_slice(b"\r\n");
    buf.put_slice(&body);
}

// impl io::Write for the response body
pub struct BodyWriter<'a>(pub &'a mut BytesMut);

impl<'a> io::Write for BodyWriter<'a> {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.put_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
