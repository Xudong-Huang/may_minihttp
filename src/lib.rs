extern crate bytes;
extern crate httparse;
extern crate time;

mod date;
mod request;
mod response;

use std::io;

pub use request::Request;
pub use response::Response;

use bytes::BytesMut;

// this is a kind of server
pub struct Http;

pub struct HttpCodec;

impl Decoder for HttpCodec {
    fn decode(&mut self, buf: &mut BytesMut) -> io::Result<Option<Request>> {
        request::decode(buf)
    }
}

impl Encoder for HttpCodec {
    fn encode(&mut self, msg: Response, buf: &mut BytesMut) -> io::Result<()> {
        response::encode(msg, buf);
        Ok(())
    }
}
