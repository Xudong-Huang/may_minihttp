use std::io::IoSlice;
use std::io::Write;

use arrayvec::ArrayVec;
use bytes::Bytes;

pub const MAX_VEC_BUF: usize = 64;

pub struct VecBufs {
    block: usize,
    pos: usize,
    bufs: ArrayVec<[Bytes; MAX_VEC_BUF]>,
}

impl VecBufs {
    pub fn new(bufs: ArrayVec<[Bytes; MAX_VEC_BUF]>) -> Self {
        VecBufs {
            block: 0,
            pos: 0,
            bufs,
        }
    }

    fn get_io_slice(&self) -> ArrayVec<[IoSlice<'_>; MAX_VEC_BUF]> {
        let mut ret = ArrayVec::new();
        let first = IoSlice::new(&self.bufs[self.block][self.pos..]);
        ret.push(first);
        for buf in self.bufs.iter().skip(self.block + 1) {
            ret.push(IoSlice::new(buf))
        }
        ret
    }

    fn advance(&mut self, n: usize) {
        let mut left = n;
        for buf in self.bufs[self.block..].iter() {
            let len = buf.len() - self.pos;
            if left >= len {
                left -= len;
                self.block += 1;
                self.pos = 0;
            } else {
                self.pos += left;
                break;
            }
        }
    }

    fn is_empty(&self) -> bool {
        self.block == self.bufs.len()
    }

    // write all data from the vecs to the writer
    pub fn write_all<W: Write>(mut self, writer: &mut W) -> std::io::Result<()> {
        while !self.is_empty() {
            let io_vec = self.get_io_slice();
            let n = writer.write_vectored(&io_vec)?;
            std::mem::forget(io_vec);
            self.advance(n);
        }
        Ok(())
    }
}
