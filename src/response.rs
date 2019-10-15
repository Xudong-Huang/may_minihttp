use std::fmt;
use std::io;
use std::ops::{Deref, DerefMut};
use std::ptr;

use bytes::{BufMut, BytesMut};

pub struct Response<'a> {
    headers: [&'static str; 16],
    headers_len: usize,
    status_message: StatusMessage,
    buf: &'a mut BytesMut,
    check_point: usize,
    body_pos: usize,
    len_buf: *mut u8,
}

pub struct Body<'a, 'b> {
    rsp: &'b mut Response<'a>,
}

struct StatusMessage {
    code: &'static str,
    msg: &'static str,
}

impl<'a> Response<'a> {
    pub(crate) fn new(buf: &'a mut BytesMut) -> Response {
        Response {
            headers: unsafe { std::mem::MaybeUninit::uninit().assume_init() },
            headers_len: 0,
            status_message: StatusMessage {
                code: "200",
                msg: "Ok",
            },
            check_point: buf.len(),
            buf,
            body_pos: 0,
            len_buf: ptr::null_mut(),
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

    pub fn get_body<'b>(&'b mut self) -> Body<'a, 'b> {
        self.encode_header();
        self.body_pos = self.buf.len();
        Body { rsp: self }
    }

    fn encode_header(&mut self) {
        self.check_point = self.buf.len();
        if self.status_message.msg == "Ok" {
            self.buf
                .put_slice(b"HTTP/1.1 200 Ok\r\nServer: may\r\nDate: ");
        } else {
            self.buf.put_slice(b"HTTP/1.1 ");
            self.buf.put_slice(self.status_message.code.as_bytes());
            self.buf.put_slice(b" ");
            self.buf.put_slice(self.status_message.msg.as_bytes());
            self.buf.put_slice(b"\r\nServer: may\r\nDate: ");
        }
        crate::date::now().put_bytes(self.buf);
        self.buf.put_slice(b"\r\nContent-Length:     ");
        self.len_buf = unsafe { self.buf.as_mut_ptr().offset((self.buf.len() - 4) as isize) };

        for i in 0..self.headers_len {
            let h = *unsafe { self.headers.get_unchecked(i) };
            self.buf.put_slice(b"\r\n");
            self.buf.put_slice(h.as_bytes());
        }

        self.buf.put_slice(b"\r\n\r\n");
    }

    pub(crate) fn encode(&mut self) {
        if self.len_buf.is_null() {
            let _ = self.get_body();
        }
        // this only write the content Length
        let content_len = self.buf.len() - self.body_pos;
        debug_assert!(content_len < 9999);
        let len_buf = ContentLenBuf(self.len_buf);
        itoa::fmt(len_buf, content_len).unwrap();
    }

    pub(crate) fn reset_buf(&mut self) {
        unsafe { self.buf.set_len(self.check_point) }
    }
}

impl<'a, 'b> Deref for Body<'a, 'b> {
    type Target = BytesMut;

    #[inline]
    fn deref(&self) -> &Self::Target {
        self.rsp.buf
    }
}

impl<'a, 'b> DerefMut for Body<'a, 'b> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.rsp.buf
    }
}

impl<'a, 'b> io::Write for Body<'a, 'b> {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.rsp.buf.put_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct ContentLenBuf(*mut u8);

impl fmt::Write for ContentLenBuf {
    #[inline]
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let len = s.len();
        unsafe {
            std::ptr::copy_nonoverlapping(s.as_ptr(), self.0, len);
            self.0.offset(len as isize);
        }
        Ok(())
    }

    #[inline]
    fn write_char(&mut self, c: char) -> fmt::Result {
        unsafe {
            *self.0 = c as u8;
            self.0.offset(1);
        }
        Ok(())
    }
}
