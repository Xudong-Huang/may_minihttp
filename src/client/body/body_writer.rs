use std::cell::RefCell;
use std::fmt;
use std::io::{self, Write};
use std::rc::Rc;

use super::BodyWriter::*;

const MAX_DROP_PADDING: usize = 64 * 1024;

#[allow(clippy::enum_variant_names)]
pub enum BodyWriter {
    SizedWriter(Rc<RefCell<dyn Write>>, usize),
    ChunkWriter(Rc<RefCell<dyn Write>>),
    // this is used to write all the data out when get drop
    EmptyWriter(Rc<RefCell<dyn Write>>),
    // this is used as a invalid place holder
    InvalidWriter,
}

impl fmt::Debug for BodyWriter {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        let name = match *self {
            SizedWriter(..) => "SizedWriter",
            ChunkWriter(_) => "ChunkWriter",
            EmptyWriter(_) => "EmptyWriter",
            InvalidWriter => "Invalid",
        };
        write!(f, "BodyWriter {}", name)
    }
}

impl Write for BodyWriter {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        use std::cmp;
        match *self {
            SizedWriter(ref w, ref mut remain) => {
                let len = cmp::min(*remain, buf.len());
                let mut w = w.borrow_mut();
                let n = w.write(&buf[0..len])?;
                *remain -= n;
                Ok(n)
            }
            ChunkWriter(ref w) => {
                let chunk_size = buf.len();
                let mut w = w.borrow_mut();
                write!(w, "{:X}\r\n", chunk_size)?;
                w.write_all(buf)?;
                w.write_all(b"\r\n")?;
                Ok(chunk_size)
            }
            EmptyWriter(_) => Ok(0),
            InvalidWriter => unreachable!(),
        }
    }

    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        match *self {
            SizedWriter(ref w, _) => {
                let mut w = w.borrow_mut();
                w.flush()
            }
            ChunkWriter(ref w) => {
                let mut w = w.borrow_mut();
                w.flush()
            }
            EmptyWriter(ref w) => {
                let mut w = w.borrow_mut();
                w.flush()
            }
            InvalidWriter => unreachable!(),
        }
    }
}

impl Drop for BodyWriter {
    fn drop(&mut self) {
        match *self {
            SizedWriter(ref w, remain) => {
                let mut w = w.borrow_mut();
                if remain > 0 && remain <= MAX_DROP_PADDING {
                    // write enough data when drop — stack buffer chunks, no heap alloc (JSF 206)
                    let zero = [0u8; 256];
                    let mut left = remain;
                    while left > 0 {
                        let amt = left.min(zero.len());
                        w.write_all(&zero[..amt]).ok();
                        left -= amt;
                    }
                }
                w.flush().ok();
            }
            ChunkWriter(ref w) => {
                // write the chunk end and flush
                let mut w = w.borrow_mut();
                w.write_all(b"0\r\n\r\n").ok();
                w.flush().ok();
            }
            EmptyWriter(ref w) => {
                let mut w = w.borrow_mut();
                w.flush().ok();
            }
            InvalidWriter => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::io;
    use std::rc::Rc;

    use super::*;

    struct CaptureWriter {
        buf: Vec<u8>,
    }

    impl io::Write for CaptureWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.buf.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    // --- BodyWriter tests ---

    #[test]
    fn test_sized_writer_exact_bytes() {
        let cw = Rc::new(RefCell::new(CaptureWriter { buf: Vec::new() }));
        let mut bw = BodyWriter::SizedWriter(cw.clone(), 7);
        assert_eq!(bw.write(b"hello\n!").unwrap(), 7);
        assert_eq!(cw.borrow().buf.as_slice(), b"hello\n!");
    }

    #[test]
    fn test_sized_writer_over_limit() {
        let cw = Rc::new(RefCell::new(CaptureWriter { buf: Vec::new() }));
        let mut bw = BodyWriter::SizedWriter(cw.clone(), 5);
        assert_eq!(bw.write(b"hello world").unwrap(), 5);
        assert_eq!(cw.borrow().buf.as_slice(), b"hello");
    }

    #[test]
    fn test_sized_writer_drop_fills_padding() {
        let cw = Rc::new(RefCell::new(CaptureWriter { buf: Vec::new() }));
        let mut bw = BodyWriter::SizedWriter(cw.clone(), 10);
        bw.write(b"hi").unwrap();
        drop(bw);
        let captured = cw.borrow().buf.clone();
        assert_eq!(captured.len(), 10);
        assert_eq!(&captured[..2], b"hi");
        assert_eq!(&captured[2..], &[0u8; 8]);
    }

    #[test]
    fn test_sized_writer_drop_does_not_pad_unbounded_length() {
        let cw = Rc::new(RefCell::new(CaptureWriter { buf: Vec::new() }));
        let bw = BodyWriter::SizedWriter(cw.clone(), MAX_DROP_PADDING + 1);
        drop(bw);
        assert!(cw.borrow().buf.is_empty());
    }

    #[test]
    fn test_chunk_writer_format() {
        let cw = Rc::new(RefCell::new(CaptureWriter { buf: Vec::new() }));
        let mut bw = BodyWriter::ChunkWriter(cw.clone());
        bw.write(b"hello").unwrap();
        assert_eq!(cw.borrow().buf.as_slice(), b"5\r\nhello\r\n");
    }

    #[test]
    fn test_chunk_writer_multiple_writes() {
        let cw = Rc::new(RefCell::new(CaptureWriter { buf: Vec::new() }));
        let mut bw = BodyWriter::ChunkWriter(cw.clone());
        bw.write(b"hello").unwrap();
        bw.write(b"world").unwrap();
        assert_eq!(cw.borrow().buf.as_slice(), b"5\r\nhello\r\n5\r\nworld\r\n");
    }

    #[test]
    fn test_chunk_writer_drop_terminator() {
        let cw = Rc::new(RefCell::new(CaptureWriter { buf: Vec::new() }));
        let mut bw = BodyWriter::ChunkWriter(cw.clone());
        bw.write(b"test").unwrap();
        drop(bw);
        let captured = cw.borrow().buf.clone();
        assert!(
            captured.ends_with(b"0\r\n\r\n"),
            "expected chunk terminator in {captured:?}"
        );
    }

    #[test]
    fn test_empty_writer_accepts_no_data() {
        let cw = Rc::new(RefCell::new(CaptureWriter { buf: Vec::new() }));
        let mut bw = BodyWriter::EmptyWriter(cw.clone());
        assert_eq!(bw.write(b"anything").unwrap(), 0);
        assert!(cw.borrow().buf.is_empty());
    }
}
