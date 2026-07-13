use std::cell::RefCell;
use std::fmt;
use std::io::{self, Read};
use std::rc::Rc;

use super::BodyReader::*;

#[allow(clippy::enum_variant_names)]
pub enum BodyReader {
    SizedReader(Rc<RefCell<dyn Read>>, usize),
    ChunkReader(Rc<RefCell<dyn Read>>, Option<usize>),
    EmptyReader,
}

impl fmt::Debug for BodyReader {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        let name = match *self {
            SizedReader(..) => "SizedReader",
            ChunkReader(..) => "ChunkReader",
            EmptyReader => "EmptyReader",
        };
        write!(f, "BodyReader {}", name)
    }
}

impl Read for BodyReader {
    #[inline]
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        use std::cmp;
        match *self {
            SizedReader(ref r, ref mut remain) => {
                let len = cmp::min(*remain, buf.len());
                if len == 0 {
                    return Ok(0);
                }
                let mut r = r.borrow_mut();
                let n = r.read(&mut buf[0..len])?;
                *remain -= n;
                Ok(n)
            }
            ChunkReader(ref r, ref mut opt_remaining) => {
                let mut r = r.borrow_mut();
                let mut rem = match *opt_remaining {
                    Some(ref rem) => *rem,
                    // None means we don't know the size of the next chunk
                    None => read_chunk_size(&mut *r)?,
                };
                trace!("Chunked read, remaining={:?}", rem);

                if rem == 0 {
                    if opt_remaining.is_none() {
                        eat(&mut *r, b"\r\n")?;
                    }

                    *opt_remaining = Some(0);

                    trace!("end of chunked");

                    return Ok(0);
                }

                let to_read = cmp::min(rem, buf.len());
                let count = r.read(&mut buf[..to_read])?;

                if count == 0 {
                    *opt_remaining = Some(0);
                    return Err(io::Error::other("early eof"));
                }

                rem -= count;
                *opt_remaining = if rem > 0 {
                    Some(rem)
                } else {
                    eat(&mut *r, b"\r\n")?;
                    None
                };
                Ok(count)
            }
            EmptyReader => Ok(0),
        }
    }
}

impl Drop for BodyReader {
    fn drop(&mut self) {
        // consume all remaining chunks — stack buffer, no heap alloc (JSF 206)
        let mut buf = [0u8; 4096];
        loop {
            match self.read(&mut buf) {
                Err(e) => {
                    error!("drop Reader err={}", e);
                    break;
                }
                Ok(n) => {
                    if n == 0 {
                        break;
                    }
                }
            }
        }
    }
}

fn eat(rdr: &mut dyn Read, bytes: &[u8]) -> io::Result<()> {
    let mut buf = [0];
    for &b in bytes.iter() {
        match rdr.read(&mut buf)? {
            1 if buf[0] == b => {}
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Invalid characters found",
                ));
            }
        }
    }
    Ok(())
}

/// Chunked chunks start with 1*HEXDIGIT, indicating the size of the chunk.
fn read_chunk_size(rdr: &mut dyn Read) -> io::Result<usize> {
    macro_rules! byte (
        ($rdr:ident) => ({
            let mut buf = [0];
            match $rdr.read(&mut buf)? {
                1 => buf[0],
                _ => return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Invalid chunk size line, read byte",
                )),
            }
        })
    );
    let mut size = 0;
    let mut in_ext = false;
    let mut in_chunk_size = true;
    loop {
        match byte!(rdr) {
            b @ b'0'..=b'9' if in_chunk_size => {
                size <<= 4;
                size += (b - b'0') as usize;
            }
            b @ b'a'..=b'f' if in_chunk_size => {
                size <<= 4;
                size += (b + 10 - b'a') as usize;
            }
            b @ b'A'..=b'F' if in_chunk_size => {
                size <<= 4;
                size += (b + 10 - b'A') as usize;
            }
            b'\r' => match byte!(rdr) {
                b'\n' => break,
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "Invalid chunk size line, read new line",
                    ));
                }
            },
            b';' if !in_ext => {
                in_ext = true;
                in_chunk_size = false;
            }
            b'\t' | b' ' if !in_ext & !in_chunk_size => {}
            b'\t' | b' ' if in_chunk_size => in_chunk_size = false,
            ext if in_ext => {
                error!("chunk extension byte={}", ext);
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Invalid chunk size line, unknown byte",
                ));
            }
        }
    }
    trace!("chunk size={:?}", size);
    Ok(size)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::io;
    use std::rc::Rc;

    use super::*;

    struct TestReader {
        data: Vec<u8>,
        pos: usize,
    }

    impl Read for TestReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let remaining = &self.data[self.pos..];
            let len = std::cmp::min(buf.len(), remaining.len());
            if len == 0 {
                return Ok(0);
            }
            buf[..len].copy_from_slice(&remaining[..len]);
            self.pos += len;
            Ok(len)
        }
    }

    // --- eat tests ---

    #[test]
    fn test_eat_valid() {
        let reader = &mut TestReader {
            data: b"\r\nhello".to_vec(),
            pos: 0,
        };
        eat(reader, b"\r\n").unwrap();
    }

    #[test]
    fn test_eat_invalid() {
        let reader = &mut TestReader {
            data: b"XXhello".to_vec(),
            pos: 0,
        };
        assert!(eat(reader, b"\r\n").is_err());
    }

    // --- read_chunk_size tests ---

    #[test]
    fn test_read_chunk_size_basic() {
        let reader = &mut TestReader {
            data: b"FF\r\n".to_vec(),
            pos: 0,
        };
        assert_eq!(read_chunk_size(reader).unwrap(), 255);
    }

    #[test]
    fn test_read_chunk_size_small() {
        let reader = &mut TestReader {
            data: b"5\r\n".to_vec(),
            pos: 0,
        };
        assert_eq!(read_chunk_size(reader).unwrap(), 5);
    }

    #[test]
    fn test_read_chunk_size_with_extension() {
        let reader = &mut TestReader {
            data: b"5;ext=val\r\n".to_vec(),
            pos: 0,
        };
        assert_eq!(read_chunk_size(reader).unwrap(), 5);
    }

    #[test]
    fn test_read_chunk_size_zero() {
        let reader = &mut TestReader {
            data: b"0\r\n\r\n".to_vec(),
            pos: 0,
        };
        assert_eq!(read_chunk_size(reader).unwrap(), 0);
    }

    #[test]
    fn test_read_chunk_size_invalid() {
        let reader = &mut TestReader {
            data: b"ZZ\r\n".to_vec(),
            pos: 0,
        };
        assert!(read_chunk_size(reader).is_err());
    }

    // --- BodyReader tests ---

    #[test]
    fn test_sized_reader_exact_bytes() {
        let reader = Rc::new(RefCell::new(TestReader {
            data: b"hello world!".to_vec(),
            pos: 0,
        }));
        let mut br = BodyReader::SizedReader(reader, 12);
        let mut buf = [0u8; 12];
        assert_eq!(br.read(&mut buf).unwrap(), 12);
        assert_eq!(&buf, b"hello world!");
        let mut buf2 = [0u8; 4];
        assert_eq!(br.read(&mut buf2).unwrap(), 0);
    }

    #[test]
    fn test_sized_reader_zero_remain() {
        let reader = Rc::new(RefCell::new(TestReader {
            data: b"nope".to_vec(),
            pos: 0,
        }));
        let mut br = BodyReader::SizedReader(reader, 0);
        let mut buf = [0u8; 4];
        assert_eq!(br.read(&mut buf).unwrap(), 0);
    }

    #[test]
    fn test_chunk_reader_multiple_chunks() {
        // 5\r\nhello\r\n5\r\nworld\r\n0\r\n\r\n
        let data = b"5\r\nhello\r\n5\r\nworld\r\n0\r\n\r\n";
        let reader = Rc::new(RefCell::new(TestReader {
            data: data.to_vec(),
            pos: 0,
        }));
        let mut br = BodyReader::ChunkReader(reader, None);
        let mut buf = [0u8; 10];
        // First read: chunk size 5, body "hello"
        assert_eq!(br.read(&mut buf).unwrap(), 5);
        assert_eq!(&buf[..5], b"hello");
        // Second read: chunk size 5, body "world"
        assert_eq!(br.read(&mut buf).unwrap(), 5);
        assert_eq!(&buf[..5], b"world");
        // Exhausted
        assert_eq!(br.read(&mut buf).unwrap(), 0);
    }

    #[test]
    fn test_chunk_reader_chunk_extensions() {
        // 5;ext=val\r\nhello\r\n0\r\n\r\n
        let data = b"5;ext=val\r\nhello\r\n0\r\n\r\n";
        let reader = Rc::new(RefCell::new(TestReader {
            data: data.to_vec(),
            pos: 0,
        }));
        let mut br = BodyReader::ChunkReader(reader, None);
        let mut buf = [0u8; 5];
        assert_eq!(br.read(&mut buf).unwrap(), 5);
        assert_eq!(&buf, b"hello");
        assert_eq!(br.read(&mut buf).unwrap(), 0);
    }

    #[test]
    fn test_chunk_reader_early_eof() {
        let data = b"10\r\nhel";
        let reader = Rc::new(RefCell::new(TestReader {
            data: data.to_vec(),
            pos: 0,
        }));
        let mut br = BodyReader::ChunkReader(reader, None);
        let mut buf = [0u8; 10];
        assert_eq!(br.read(&mut buf).unwrap(), 3);
        assert!(br.read(&mut buf).is_err());
    }

    #[test]
    fn test_empty_reader_always_zero() {
        let mut br = BodyReader::EmptyReader;
        let mut buf = [0u8; 4];
        assert_eq!(br.read(&mut buf).unwrap(), 0);
    }

    #[test]
    fn test_drop_consumes_remaining_chunks() {
        // 5\r\nhello\r\n3\r\nabc\r\n
        let data = b"5\r\nhello\r\n3\r\nabc\r\n";
        let reader = Rc::new(RefCell::new(TestReader {
            data: data.to_vec(),
            pos: 0,
        }));
        let mut br = BodyReader::ChunkReader(reader, None);
        let mut buf = [0u8; 10];
        assert_eq!(br.read(&mut buf).unwrap(), 5);
        assert_eq!(&buf[..5], b"hello");
        assert_eq!(br.read(&mut buf).unwrap(), 3);
        assert_eq!(&buf[..3], b"abc");
        drop(br); // should not panic
    }
}
