use std::cmp;
use std::io::{self, BufRead, Read, Write};

use bytes::{Buf, BufMut, BytesMut};

#[derive(Debug)]
pub struct BufferIo<T> {
    inner: T,
    reader_buf: BytesMut,
    writer_buf: (Vec<u8>, usize),
}

const INIT_BUFFER_SIZE: usize = 4096;

impl<T> BufferIo<T> {
    #[inline]
    pub fn new(io: T) -> Self {
        BufferIo::with_capacity(io, INIT_BUFFER_SIZE)
    }

    #[inline]
    pub fn with_capacity(io: T, cap: usize) -> Self {
        BufferIo {
            inner: io,
            reader_buf: BytesMut::with_capacity(cap),
            writer_buf: (vec![0u8; cap], 0),
        }
    }

    #[inline]
    pub fn inner_mut(&mut self) -> &mut T {
        &mut self.inner
    }
}

impl<T: Read> BufferIo<T> {
    /// read some data into internal buffer
    #[inline]
    pub fn bump_read(&mut self) -> io::Result<usize> {
        if self.reader_buf.capacity() - self.reader_buf.len() < 32 {
            self.reader_buf.reserve(INIT_BUFFER_SIZE);
        }

        let spare = self.reader_buf.spare_capacity_mut();
        let buf =
            unsafe { std::slice::from_raw_parts_mut(spare.as_mut_ptr() as *mut u8, spare.len()) };
        let n = self.inner.read(buf)?;
        // SAFETY: `read` initialized exactly `n` bytes at the start of spare capacity.
        unsafe {
            self.reader_buf.advance_mut(n);
        }
        Ok(n)
    }

    /// return the internal buffer
    #[inline]
    pub fn get_reader_buf(&mut self) -> &mut BytesMut {
        &mut self.reader_buf
    }
}

impl<T: Read> Read for BufferIo<T> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        use std::ptr;
        if self.reader_buf.is_empty() {
            self.bump_read()?;
        }

        let len = unsafe {
            let src = self.reader_buf.as_ref();
            let len = cmp::min(buf.len(), src.len());
            ptr::copy_nonoverlapping(src.as_ptr(), buf.as_mut_ptr(), len);
            len
        };

        self.reader_buf.advance(len);
        Ok(len)
    }
}

impl<T: Write> Write for BufferIo<T> {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        use std::ptr;
        let buf_len = self.writer_buf.0.len();
        if buf.len() >= buf_len {
            self.flush()?;
            return self.inner.write(buf);
        }

        if buf_len == self.writer_buf.1 {
            self.flush()?;
        }

        let remain = buf_len - self.writer_buf.1;
        let len = cmp::min(remain, buf.len());
        let dst = self.writer_buf.0.as_mut_ptr();
        unsafe {
            let dst = dst.add(self.writer_buf.1);
            ptr::copy_nonoverlapping(buf.as_ptr(), dst, len);
        }
        self.writer_buf.1 += len;
        Ok(len)
    }

    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        let buf = &self.writer_buf.0[0..self.writer_buf.1];
        self.inner.write_all(buf)?;
        self.writer_buf.1 = 0;
        Ok(())
    }
}

impl<T: Read> BufRead for BufferIo<T> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        self.bump_read()?;
        Ok(self.reader_buf.chunk())
    }

    #[inline]
    fn consume(&mut self, amt: usize) {
        self.reader_buf.advance(amt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, BufRead, Read, Write};

    #[derive(Default)]
    struct RecordingWriter {
        writes: Vec<Vec<u8>>,
    }

    impl Write for RecordingWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.writes.push(buf.to_vec());
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    struct SlowRead(u8);

    impl Read for SlowRead {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let state = self.0;
            self.0 += 1;
            (&match state % 3 {
                0 => b"foo",
                1 => b"bar",
                _ => b"baz",
            }[..])
                .read(buf)
        }
    }

    #[test]
    fn test_consume_and_get_buf() {
        let mut rdr = BufferIo::new(SlowRead(0));
        rdr.bump_read().unwrap();
        rdr.consume(1);
        assert_eq!(rdr.get_reader_buf().as_ref(), b"oo");
        rdr.bump_read().unwrap();
        rdr.bump_read().unwrap();
        assert_eq!(rdr.get_reader_buf().as_ref(), b"oobarbaz");
        rdr.consume(5);
        assert_eq!(rdr.get_reader_buf().as_ref(), b"baz");
        rdr.consume(3);
        assert_eq!(rdr.get_reader_buf().as_ref(), b"");
    }

    #[test]
    fn test_resize() {
        let raw = vec![1u8; 100];
        let mut rdr = BufferIo::with_capacity(&raw[..], 65);
        rdr.bump_read().unwrap();
        assert_eq!(rdr.get_reader_buf().len(), 65);
        rdr.bump_read().unwrap();
        assert_eq!(rdr.get_reader_buf().len(), 100);
    }

    #[test]
    fn test_write() {
        let data = vec![0u8; 100];
        let mut wrt = BufferIo::with_capacity(io::sink(), 40);
        let n = wrt.write(&data).unwrap();
        assert_eq!(n, 100);
        let n = wrt.write(&[0u8; 6]).unwrap();
        assert_eq!(n, 6);
        let n = wrt.write(&data).unwrap();
        assert_eq!(n, 100);
        let n = wrt.write(&data).unwrap();
        assert_eq!(n, 100);
    }

    #[test]
    fn large_write_flushes_buffer_then_bypasses_it() {
        let mut writer = BufferIo::with_capacity(RecordingWriter::default(), 4);
        writer.write_all(b"ab").unwrap();
        writer.write_all(b"01234567").unwrap();

        assert_eq!(
            writer.inner.writes,
            vec![b"ab".to_vec(), b"01234567".to_vec()]
        );
        assert_eq!(writer.writer_buf.1, 0);
    }
}
