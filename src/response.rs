use std::io;

use bytes::{BufMut, BytesMut};
use itoa;

pub struct Response {
    headers: [(&'static str, &'static str); 16],
    headers_len: usize,
    status_message: StatusMessage,
    body: Body,
}

enum Body {
    SMsg(&'static str),
    DMsg(BytesMut),
}

impl Body {
    fn len(&self) -> usize {
        match self {
            Body::SMsg(s) => s.len(),
            Body::DMsg(v) => v.len(),
        }
    }

    fn as_bytes(&self) -> &[u8] {
        match self {
            Body::SMsg(s) => s.as_bytes(),
            Body::DMsg(v) => &v,
        }
    }
}

struct StatusMessage {
    code: &'static str,
    msg: &'static str,
}

impl Response {
    pub fn new() -> Response {
        Response {
            headers: unsafe { std::mem::MaybeUninit::uninit().assume_init() },
            headers_len: 0,
            body: Body::DMsg(BytesMut::new()),
            status_message: StatusMessage {
                code: "200",
                msg: "Ok",
            },
        }
    }

    pub fn status_code(&mut self, code: &'static str, msg: &'static str) -> &mut Response {
        self.status_message = StatusMessage { code, msg };
        self
    }

    pub fn header(&mut self, name: &'static str, val: &'static str) -> &mut Response {
        self.headers[self.headers_len] = (name, val);
        self.headers_len += 1;
        self
    }

    pub fn body(&mut self, s: &'static str) -> &mut Response {
        self.body = Body::SMsg(s);
        self
    }

    pub fn body_mut(&mut self) -> BodyWriter {
        let buf = match self.body {
            Body::DMsg(ref mut v) => return BodyWriter(v),
            Body::SMsg(s) => {
                let mut buf = BytesMut::new();
                if !s.is_empty() {
                    buf.extend_from_slice(s.as_bytes());
                }
                buf
            }
        };

        self.body = Body::DMsg(buf);
        match &mut self.body {
            Body::DMsg(v) => BodyWriter(v),
            Body::SMsg(_) => unreachable!(),
        }
    }
}

pub fn encode(msg: Response, mut buf: &mut BytesMut) {
    let length = msg.body.len();
    buf.put_slice(b"HTTP/1.1 ");
    buf.put_slice(msg.status_message.code.as_bytes());
    buf.put_u8(b' ');
    buf.put_slice(msg.status_message.msg.as_bytes());
    buf.put_slice(b"\r\nServer: may\r\nDate: ");
    ::date::now().put_bytes(buf);
    buf.put_slice(b"\r\nContent-Length: ");
    itoa::fmt(&mut buf, length).unwrap();
    buf.put_slice(b"\r\n");

    for i in 0..msg.headers_len {
        let (k, v) = msg.headers[i];
        buf.put_slice(k.as_bytes());
        buf.put_slice(b": ");
        buf.put_slice(v.as_bytes());
        buf.put_slice(b"\r\n");
    }

    buf.put_slice(b"\r\n");
    buf.put_slice(msg.body.as_bytes());
}

// TODO: impl fmt::Write for Vec<u8>
//
// Right now `write!` on `Vec<u8>` goes through io::Write and is not super
// speedy, so inline a less-crufty implementation here which doesn't go through
// io::Error.
pub struct BodyWriter<'a>(pub &'a mut BytesMut);

impl<'a> io::Write for BodyWriter<'a> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
