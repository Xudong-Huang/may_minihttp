use std::fmt;
use std::io::{self, BufRead, Read};
use std::mem::MaybeUninit;

/// Maximum header buffer size configurations.
///
/// This enum provides pre-defined buffer sizes for different use cases while
/// allowing custom sizes via the `Custom` variant.
///
/// # Examples
///
/// ```
/// use may_minihttp::MaxHeaders;
///
/// let default = MaxHeaders::Default;
/// assert_eq!(default.value(), 16);
///
/// let large = MaxHeaders::Large;
/// assert_eq!(large.value(), 64);
///
/// let custom = MaxHeaders::Custom(100);
/// assert_eq!(custom.value(), 100);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Maximum number of HTTP header lines to accept in a single request.
///
/// This enum controls how many **individual header lines** the parser will accept.
/// Each variant represents a specific count of header items (not bytes or kilobytes).
///
/// # What Counts as a Header?
///
/// Each line in the HTTP request counts as one header:
/// ```http
/// GET /api HTTP/1.1
/// Host: example.com              ← Header 1
/// User-Agent: Mozilla/5.0        ← Header 2
/// Accept: application/json       ← Header 3
/// X-Custom-Header: value         ← Header 4
/// ```
///
/// # Choosing the Right Size
///
/// | Variant | Count | Use Case | Example Environment |
/// |---------|-------|----------|---------------------|
/// | `Default` | 16 | Simple APIs, controlled environments | Direct API calls, testing |
/// | `Standard` | 32 | Most web applications | Standard web apps behind single proxy |
/// | `Large` | 64 | Complex deployments | Multiple proxies, load balancers |
/// | `XLarge` | 128 | Production infrastructure | Kubernetes, service mesh, multiple proxy layers |
///
/// # Real-World Header Counts
///
/// - **Direct browser request**: 10-15 headers
/// - **Browser + CDN**: 15-25 headers
/// - **Browser + CDN + Load Balancer**: 25-40 headers
/// - **Kubernetes (Ingress + Service Mesh)**: 40-80 headers
/// - **Full enterprise stack**: 60-100+ headers
///
/// # Memory Impact
///
/// Each header slot consumes a small amount of stack memory (typically ~24 bytes for
/// the header reference). Increasing the limit has minimal memory overhead.
///
/// # Examples
///
/// ```rust
/// use may_minihttp::MaxHeaders;
///
/// // For a simple API with direct clients
/// let simple = MaxHeaders::Default;  // 16 header items
/// assert_eq!(simple.value(), 16);
///
/// // For production Kubernetes deployment
/// let production = MaxHeaders::XLarge;  // 128 header items
/// assert_eq!(production.value(), 128);
///
/// // Custom size (clamped to 1-256 range)
/// let custom = MaxHeaders::Custom(96);  // 96 header items
/// assert_eq!(custom.value(), 96);
/// ```
#[derive(Default)]
pub enum MaxHeaders {
    /// Default: 16 header items (backwards compatible, minimal memory)
    ///
    /// Accepts up to **16 individual HTTP header lines**.
    ///
    /// **Suitable for**: Simple APIs, controlled environments, testing
    #[default]
    Default,

    /// Standard: 32 header items
    ///
    /// Accepts up to **32 individual HTTP header lines**.
    ///
    /// **Suitable for**: Most web applications, single proxy/load balancer
    Standard,

    /// Large: 64 header items
    ///
    /// Accepts up to **64 individual HTTP header lines**.
    ///
    /// **Suitable for**: Applications behind load balancers, CDN + proxy
    Large,

    /// `XLarge`: 128 header items
    ///
    /// Accepts up to **128 individual HTTP header lines**.
    ///
    /// **Suitable for**: Production services in Kubernetes, service mesh,
    /// multiple proxy layers, enterprise environments with extensive headers
    XLarge,

    /// Custom size: 1-256 header items
    ///
    /// Accepts up to the specified number of **individual HTTP header lines**.
    ///
    /// Values are automatically clamped:
    /// - Minimum: 16 (if 0 is specified)
    /// - Maximum: 256
    ///
    /// # Example
    /// ```rust
    /// use may_minihttp::MaxHeaders;
    ///
    /// let custom = MaxHeaders::Custom(96);  // 96 header items
    /// assert_eq!(custom.value(), 96);
    ///
    /// let clamped_low = MaxHeaders::Custom(0);  // Clamped to 16
    /// assert_eq!(clamped_low.value(), 16);
    ///
    /// let clamped_high = MaxHeaders::Custom(512);  // Clamped to 256
    /// assert_eq!(clamped_high.value(), 256);
    /// ```
    Custom(usize),
}

impl MaxHeaders {
    /// Get the numeric value of the max headers setting
    #[must_use]
    pub const fn value(&self) -> usize {
        match self {
            MaxHeaders::Default => 16,
            MaxHeaders::Standard => 32,
            MaxHeaders::Large => 64,
            MaxHeaders::XLarge => 128,
            MaxHeaders::Custom(n) => {
                // Clamp to reasonable range
                if *n == 0 {
                    16
                } else if *n > 256 {
                    256
                } else {
                    *n
                }
            }
        }
    }
}

/// Default maximum number of HTTP headers (backwards compatible)
pub(crate) const MAX_HEADERS: usize = MaxHeaders::Default.value();

use bytes::{Buf, BufMut, BytesMut};
use may::net::TcpStream;

use crate::http_server::err;

pub struct BodyReader<'buf, 'stream> {
    // remaining bytes for body
    req_buf: &'buf mut BytesMut,
    // the max body length limit
    body_limit: usize,
    // total read count
    total_read: usize,
    // used to read extra body bytes
    stream: &'stream mut TcpStream,
}

impl BodyReader<'_, '_> {
    fn read_more_data(&mut self) -> io::Result<usize> {
        crate::http_server::reserve_buf(self.req_buf);
        let read_buf: &mut [u8] = unsafe { std::mem::transmute(self.req_buf.chunk_mut()) };
        let n = self.stream.read(read_buf)?;
        unsafe { self.req_buf.advance_mut(n) };
        Ok(n)
    }
}

impl Read for BodyReader<'_, '_> {
    // the user should control the body reading, don't exceeds the body!
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.total_read >= self.body_limit {
            return Ok(0);
        }

        loop {
            if !self.req_buf.is_empty() {
                let min_len = buf.len().min(self.body_limit - self.total_read);
                let n = self.req_buf.reader().read(&mut buf[..min_len])?;
                self.total_read += n;
                return Ok(n);
            }

            if self.read_more_data()? == 0 {
                return Ok(0);
            }
        }
    }
}

impl BufRead for BodyReader<'_, '_> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        let remain = self.body_limit - self.total_read;
        if remain == 0 {
            return Ok(&[]);
        }
        if self.req_buf.is_empty() {
            self.read_more_data()?;
        }
        let n = self.req_buf.len().min(remain);
        Ok(&self.req_buf.chunk()[0..n])
    }

    fn consume(&mut self, amt: usize) {
        assert!(amt <= self.body_limit - self.total_read);
        assert!(amt <= self.req_buf.len());
        self.total_read += amt;
        self.req_buf.advance(amt)
    }
}

impl Drop for BodyReader<'_, '_> {
    fn drop(&mut self) {
        // consume all the remaining bytes
        while let Ok(n) = self.fill_buf().map(|b| b.len()) {
            if n == 0 {
                break;
            }
            // println!("drop: {:?}", n);
            self.consume(n);
        }
    }
}

// we should hold the mut ref of req_buf
// before into body, this req_buf is only for holding headers
// after into body, this req_buf is mutable to read extra body bytes
// and the headers buf can be reused
pub struct Request<'buf, 'header, 'stream> {
    req: httparse::Request<'header, 'buf>,
    req_buf: &'buf mut BytesMut,
    stream: &'stream mut TcpStream,
}

impl<'buf, 'stream> Request<'buf, '_, 'stream> {
    pub fn method(&self) -> &str {
        self.req.method.unwrap()
    }

    pub fn path(&self) -> &str {
        self.req.path.unwrap()
    }

    pub fn version(&self) -> u8 {
        self.req.version.unwrap()
    }

    pub fn headers(&self) -> &[httparse::Header<'_>] {
        self.req.headers
    }

    pub fn body(self) -> BodyReader<'buf, 'stream> {
        BodyReader {
            body_limit: self.content_length(),
            total_read: 0,
            stream: self.stream,
            req_buf: self.req_buf,
        }
    }

    fn content_length(&self) -> usize {
        let mut len = 0;
        for header in self.req.headers.iter() {
            if header.name.eq_ignore_ascii_case("content-length") {
                len = std::str::from_utf8(header.value).unwrap().parse().unwrap();
                break;
            }
        }
        len
    }
}

impl fmt::Debug for Request<'_, '_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "<HTTP Request {} {}>", self.method(), self.path())
    }
}

pub fn decode<'header, 'buf, 'stream, const N: usize>(
    headers: &'header mut [MaybeUninit<httparse::Header<'buf>>; N],
    req_buf: &'buf mut BytesMut,
    stream: &'stream mut TcpStream,
) -> io::Result<Option<Request<'buf, 'header, 'stream>>> {
    let mut req = httparse::Request::new(&mut []);
    // safety: don't hold the reference of req_buf
    // so we can transfer the mutable reference to Request
    let buf: &[u8] = unsafe { std::mem::transmute(req_buf.chunk()) };

    // Wait for complete headers before parsing to prevent token errors
    // This fixes issue #18 where headers arriving in multiple TCP packets
    // would cause "Token" parsing errors
    // The \r\n\r\n sequence marks the end of HTTP headers
    if !buf.windows(4).any(|window| window == b"\r\n\r\n") {
        return Ok(None); // Need more data
    }

    // Get the header limit before parsing (to avoid borrow issues)
    let header_limit = headers.len();

    let status = match req.parse_with_uninit_headers(buf, headers) {
        Ok(s) => s,
        Err(e) => {
            // Provide detailed error message for TooManyHeaders
            let msg = if e == httparse::Error::TooManyHeaders {
                // Count how many headers were actually sent
                let header_count = buf
                    .split(|&b| b == b'\n')
                    .filter(|line| {
                        !line.is_empty() && line.contains(&b':') && !line.starts_with(b"\r\n")
                    })
                    .count();

                let over_by = header_count.saturating_sub(header_limit);

                let error_msg = format!(
                    "TooManyHeaders: received {header_count} headers, limit is {header_limit} (over by {over_by})"
                );

                // Log the error
                eprintln!("{error_msg}");

                // Log the suggestion on a separate line for clarity
                eprintln!(
                    "Suggestion: Consider using MaxHeaders::Standard (32), \
                     MaxHeaders::Large (64), or MaxHeaders::XLarge (128) for production deployments."
                );

                error_msg
            } else {
                let error_msg = format!("failed to parse http request: {e:?}");
                eprintln!("{error_msg}");
                error_msg
            };

            return err(io::Error::other(msg));
        }
    };

    let len = match status {
        httparse::Status::Complete(amt) => amt,
        httparse::Status::Partial => return Ok(None),
    };
    req_buf.advance(len);

    // println!("req: {:?}", std::str::from_utf8(req_buf).unwrap());
    Ok(Some(Request {
        req,
        req_buf,
        stream,
    }))
}

/// Decode HTTP request with Default (16) headers
///
/// # Errors
///
/// Returns an error if:
/// - The TCP stream cannot be read
/// - The HTTP request is malformed
/// - The number of headers exceeds 16
pub fn decode_default<'header, 'buf, 'stream>(
    headers: &'header mut [MaybeUninit<httparse::Header<'buf>>; 16],
    req_buf: &'buf mut BytesMut,
    stream: &'stream mut TcpStream,
) -> io::Result<Option<Request<'buf, 'header, 'stream>>> {
    decode(headers, req_buf, stream)
}

/// Decode HTTP request with Standard (32) headers
///
/// # Errors
///
/// Returns an error if:
/// - The TCP stream cannot be read
/// - The HTTP request is malformed
/// - The number of headers exceeds 32
pub fn decode_standard<'header, 'buf, 'stream>(
    headers: &'header mut [MaybeUninit<httparse::Header<'buf>>; 32],
    req_buf: &'buf mut BytesMut,
    stream: &'stream mut TcpStream,
) -> io::Result<Option<Request<'buf, 'header, 'stream>>> {
    decode(headers, req_buf, stream)
}

/// Decode HTTP request with Large (64) headers
///
/// # Errors
///
/// Returns an error if:
/// - The TCP stream cannot be read
/// - The HTTP request is malformed
/// - The number of headers exceeds 64
pub fn decode_large<'header, 'buf, 'stream>(
    headers: &'header mut [MaybeUninit<httparse::Header<'buf>>; 64],
    req_buf: &'buf mut BytesMut,
    stream: &'stream mut TcpStream,
) -> io::Result<Option<Request<'buf, 'header, 'stream>>> {
    decode(headers, req_buf, stream)
}

/// Decode HTTP request with `XLarge` (128) headers
///
/// # Errors
///
/// Returns an error if:
/// - The TCP stream cannot be read
/// - The HTTP request is malformed
/// - The number of headers exceeds 128
pub fn decode_xlarge<'header, 'buf, 'stream>(
    headers: &'header mut [MaybeUninit<httparse::Header<'buf>>; 128],
    req_buf: &'buf mut BytesMut,
    stream: &'stream mut TcpStream,
) -> io::Result<Option<Request<'buf, 'header, 'stream>>> {
    decode(headers, req_buf, stream)
}
