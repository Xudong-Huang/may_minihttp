//! Coroutine HTTP/1.1 client (drop-in replacement for `may_http::client`).
//!
//! Enabled with the `client` feature. Uses native transport on `may::net::TcpStream`
//! — no dependency on the abandoned `may_http` crate.

mod body;
mod buffer;
mod client_impl;
mod request;
mod response;

pub use client_impl::HttpClient;
pub use request::Request;
pub use response::Response;
