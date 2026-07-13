use std::cell::RefCell;
use std::fmt;
use std::io::{self, Read};
use std::ops::{Deref, DerefMut};
use std::rc::Rc;

use bytes::BytesMut;
use http::header::*;
use http::{self, Version};
use httparse;

use crate::client::body::BodyReader;

pub(crate) fn decode(buf: &mut BytesMut) -> io::Result<Option<Response>> {
    // Parse into owned response metadata before mutating `buf`. `httparse`
    // stores header slices that borrow the input buffer, so splitting the
    // buffer while the parser is alive would violate Rust's aliasing rules.
    let (head_len, version, status_code, response_headers) = {
        let mut headers = [httparse::EMPTY_HEADER; 64];
        let mut parsed = httparse::Response::new(&mut headers);
        let status = parsed.parse(buf).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("failed to parse HTTP response: {e}"),
            )
        })?;

        let head_len = match status {
            httparse::Status::Complete(amount) => amount,
            httparse::Status::Partial => return Ok(None),
        };
        let version = match parsed.version {
            Some(0) => Version::HTTP_10,
            Some(_) | None => Version::HTTP_11,
        };
        let status_code = parsed.code.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "HTTP response missing status code",
            )
        })?;
        let response_headers = parsed
            .headers
            .iter()
            .map(|header| {
                let name = HeaderName::from_bytes(header.name.as_bytes()).map_err(|e| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("invalid HTTP response header name: {e}"),
                    )
                })?;
                let value = HeaderValue::from_bytes(header.value).map_err(|e| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("invalid HTTP response header value: {e}"),
                    )
                })?;
                Ok((name, value))
            })
            .collect::<io::Result<Vec<_>>>()?;

        (head_len, version, status_code, response_headers)
    };

    // The parser and all header borrows are gone, so advancing the input is safe.
    let _ = buf.split_to(head_len);

    let mut rsp_builder = http::Response::builder();
    rsp_builder = rsp_builder.status(status_code).version(version);

    for (name, value) in response_headers {
        rsp_builder = rsp_builder.header(name, value);
    }

    rsp_builder
        .body(BodyReader::EmptyReader)
        .map(|req| Some(Response(req)))
        .map_err(|e| {
            let msg = format!("failed to build http Response: {e:?}");
            io::Error::other(msg)
        })
}

/// HTTP response from a client request.
pub struct Response(http::Response<BodyReader>);

impl Response {
    pub(crate) fn set_reader(
        &mut self,
        reader: Rc<RefCell<dyn Read>>,
        expect_body: bool,
    ) -> io::Result<()> {
        if !expect_body {
            *self.body_mut() = BodyReader::EmptyReader;
            return Ok(());
        }

        use std::str;

        let size = self
            .headers()
            .get(CONTENT_LENGTH)
            .map(|v| {
                let s = unsafe { str::from_utf8_unchecked(v.as_bytes()) };
                s.parse().map_err(|e| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("malformed Content-Length: {e}"),
                    )
                })
            })
            .transpose()?;

        let body_reader = match size {
            Some(n) => BodyReader::SizedReader(reader, n),
            None => BodyReader::ChunkReader(reader, None),
        };

        *self.body_mut() = body_reader;
        Ok(())
    }
}

impl Deref for Response {
    type Target = http::Response<BodyReader>;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Response {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Read for Response {
    #[inline]
    fn read(&mut self, msg: &mut [u8]) -> io::Result<usize> {
        self.body_mut().read(msg)
    }
}

impl fmt::Debug for Response {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "<HTTP Response {} {:?}>", self.status(), self.version())
    }
}

#[cfg(test)]
mod tests {
    use bytes::BytesMut;

    use super::decode;

    fn build_response(status: u16, headers: &[(&str, &str)], body: &str) -> String {
        let mut resp = format!("HTTP/1.1 {}\r\n", status);
        for (name, value) in headers {
            resp.push_str(&format!("{}: {}\r\n", name, value));
        }
        if !body.is_empty() {
            resp.push_str(&format!("Content-Length: {}\r\n", body.len()));
        }
        resp.push_str("\r\n");
        resp.push_str(body);
        resp
    }

    #[test]
    fn test_decode_valid_200() {
        let text = build_response(200, &[("Server", "test")], "hello");
        let mut buf = BytesMut::from(text.as_bytes());
        let rsp = decode(&mut buf).unwrap().unwrap();
        assert_eq!(rsp.status().as_u16(), 200);
        assert_eq!(rsp.version(), http::Version::HTTP_11);
        assert_eq!(rsp.headers()["Server"], "test");
        assert_eq!(buf.as_ref(), b"hello");
    }

    #[test]
    fn test_decode_partial() {
        let mut buf = BytesMut::from(b"HTTP/1.1 200 OK\r\nServer: t".as_slice());
        assert!(decode(&mut buf).unwrap().is_none());
    }

    #[test]
    fn test_decode_content_length() {
        let text = build_response(200, &[("Content-Length", "5")], "hello");
        let mut buf = BytesMut::from(text.as_bytes());
        let rsp = decode(&mut buf).unwrap().unwrap();
        assert!(rsp.headers().get("Content-Length").is_some());
    }

    #[test]
    fn test_decode_http10() {
        let text = "HTTP/1.0 200 OK\r\n\r\n";
        let mut buf = BytesMut::from(text.as_bytes());
        let rsp = decode(&mut buf).unwrap().unwrap();
        assert_eq!(rsp.version(), http::Version::HTTP_10);
    }

    #[test]
    fn test_decode_malformed() {
        let mut buf = BytesMut::from(b"not a response".as_slice());
        assert!(decode(&mut buf).is_err());
    }

    #[test]
    fn test_decode_set_reader_with_expect_body() {
        use std::cell::RefCell;
        use std::io::Read;
        use std::rc::Rc;

        let text = build_response(200, &[("Content-Length", "5")], "");
        let mut buf = BytesMut::from(text.as_bytes());
        let mut rsp = decode(&mut buf).unwrap().unwrap();

        struct FakeReader;
        impl Read for FakeReader {
            fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
                Ok(0)
            }
        }

        let reader = Rc::new(RefCell::new(FakeReader));
        rsp.set_reader(reader, true).unwrap();

        match rsp.body() {
            super::BodyReader::SizedReader(_, ref n) => assert_eq!(*n, 5),
            _ => panic!("expected SizedReader"),
        }
    }

    #[test]
    fn test_decode_set_reader_no_body() {
        use std::cell::RefCell;
        use std::io::Read;
        use std::rc::Rc;

        let text = build_response(200, &[] as &[(&str, &str)], "");
        let mut buf = BytesMut::from(text.as_bytes());
        let mut rsp = decode(&mut buf).unwrap().unwrap();

        struct FakeReader;
        impl Read for FakeReader {
            fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
                Ok(0)
            }
        }

        let reader = Rc::new(RefCell::new(FakeReader));
        rsp.set_reader(reader, false).unwrap();

        assert!(matches!(*rsp.body(), super::BodyReader::EmptyReader));
    }

    #[test]
    fn test_decode_set_reader_bad_cl() {
        use std::cell::RefCell;
        use std::io::Read;
        use std::rc::Rc;

        let text = build_response(200, &[("Content-Length", "abc")], "");
        let mut buf = BytesMut::from(text.as_bytes());
        let mut rsp = decode(&mut buf).unwrap().unwrap();

        struct FakeReader;
        impl Read for FakeReader {
            fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
                Ok(0)
            }
        }

        let reader = Rc::new(RefCell::new(FakeReader));
        let err = rsp.set_reader(reader, true).unwrap_err();
        assert!(err.to_string().contains("malformed Content-Length"));
    }
}
