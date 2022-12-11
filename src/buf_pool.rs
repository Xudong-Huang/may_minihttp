use bytes::BytesMut;
use may::sync::Mutex;
use once_cell::sync::Lazy;

use std::ops::{Deref, DerefMut};

const MAX_BUFS: usize = 4096;
const BUF_LEN: usize = 4096 * 8;

pub struct BytesBuf(BytesMut);

impl Deref for BytesBuf {
    type Target = BytesMut;
    fn deref(&self) -> &BytesMut {
        &self.0
    }
}

impl From<BytesMut> for BytesBuf {
    fn from(value: BytesMut) -> Self {
        Self(value)
    }
}

impl DerefMut for BytesBuf {
    fn deref_mut(&mut self) -> &mut BytesMut {
        &mut self.0
    }
}

impl Drop for BytesBuf {
    fn drop(&mut self) {
        let buf = std::mem::replace(self, BytesMut::new().into());
        BUF_POOL.put(buf)
    }
}

pub struct BufBool {
    // the pool must support mpmc operation!
    pool: Mutex<Vec<BytesBuf>>,
}

impl BufBool {
    pub fn new() -> Self {
        let capacity = MAX_BUFS;
        let mut pool = Vec::new();
        for _ in 0..capacity {
            let buf = BytesMut::with_capacity(BUF_LEN).into();
            pool.push(buf);
        }

        BufBool {
            pool: Mutex::new(pool),
        }
    }

    /// get a raw coroutine from the pool
    #[inline]
    pub fn get(&self) -> BytesBuf {
        match self.pool.lock().unwrap().pop() {
            Some(buf) => buf,
            None => BytesMut::with_capacity(BUF_LEN).into(),
        }
    }

    /// put a raw coroutine into the pool
    #[inline]
    pub fn put(&self, buf: BytesBuf) {
        let mut pool = self.pool.lock().unwrap();
        // discard the co if push failed
        if pool.len() >= MAX_BUFS {
            return;
        }
        pool.push(buf);
    }
}

pub static BUF_POOL: Lazy<BufBool> = Lazy::new(BufBool::new);

#[inline]
pub fn reserve_buf(buf: &mut BytesMut) {
    let capacity = buf.capacity();
    if capacity < 1024 {
        buf.reserve(BUF_LEN - capacity);
    }
}
