extern crate bytes;
extern crate co_managed;
extern crate httparse;
#[macro_use]
extern crate log;
#[macro_use]
extern crate may;
extern crate time;

mod date;
mod request;
mod response;
mod http_server;

pub use request::Request;
pub use response::Response;
pub use http_server::HttpServer;

// use std::io;
// use bytes::BytesMut;

// pub struct HttpCodec;

// impl Decoder for HttpCodec {
//     fn decode(&mut self, buf: &mut BytesMut) -> io::Result<Option<Request>> {
//         request::decode(buf)
//     }
// }

// impl Encoder for HttpCodec {
//     fn encode(&mut self, msg: Response, buf: &mut BytesMut) -> io::Result<()> {
//         response::encode(msg, buf);
//         Ok(())
//     }
// }
