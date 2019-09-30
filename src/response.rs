use std::io;

use crate::vec_buf::MAX_VEC_BUF;
use arrayvec::ArrayVec;
use bytes::{BufMut, Bytes, BytesMut};

pub struct Response<'a> {
    headers: [(&'static str, &'static str); 16],
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

    pub fn header(&mut self, name: &'static str, val: &'static str) -> &mut Self {
        debug_assert!(self.headers_len < 16);
        *unsafe { self.headers.get_unchecked_mut(self.headers_len) } = (name, val);
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
                self.rsp_buf.extend_from_slice(s.as_bytes());
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

// the returned buf will be reused.
pub(crate) fn encode(mut msg: Response) -> ArrayVec<[Bytes; MAX_VEC_BUF]> {
    let mut ret = ArrayVec::new();
    // must first extrac the body to reuse the buf
    let body = msg.get_body();
    let mut buf = msg.rsp_buf;

    buf.put_slice(b"HTTP/1.1 ");
    buf.put_slice(msg.status_message.code.as_bytes());
    buf.put_u8(b' ');
    buf.put_slice(msg.status_message.msg.as_bytes());
    buf.put_slice(b"\r\nServer: may\r\nDate: ");
    crate::date::now().put_bytes(buf);
    buf.put_slice(b"\r\nContent-Length: ");
    itoa::fmt(&mut buf, body.len()).unwrap();
    buf.put_slice(b"\r\n");

    for i in 0..msg.headers_len {
        let (k, v) = *unsafe { msg.headers.get_unchecked(i) };
        buf.put_slice(k.as_bytes());
        buf.put_slice(b": ");
        buf.put_slice(v.as_bytes());
        buf.put_slice(b"\r\n");
    }

    buf.put_slice(b"\r\n");

    unsafe {
        ret.push_unchecked(buf.take().freeze());
        ret.push_unchecked(body);
    }
    ret
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
