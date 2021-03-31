use std::cell::UnsafeCell;
use std::fmt::{self, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use bytes::BytesMut;
use lazy_static::lazy_static;

// "Sun, 06 Nov 1994 08:49:37 GMT".len()
const DATE_VALUE_LENGTH: usize = 29;

lazy_static! {
    static ref CURRENT_DATE: Arc<DataWrap> = {
        let date = Arc::new(DataWrap(UnsafeCell::new(Date::new())));
        let date_clone = date.clone();
        may::go!(move || loop {
            may::coroutine::sleep(std::time::Duration::from_millis(500));
            unsafe { &mut *(date_clone.0).get() }.update();
        });
        date
    };
}

struct DataWrap(UnsafeCell<Date>);
unsafe impl Sync for DataWrap {}

#[doc(hidden)]
pub fn set_date(dst: &mut BytesMut) {
    let date = unsafe { &*CURRENT_DATE.0.get() };
    dst.extend_from_slice(date.as_bytes());
}

struct Date {
    bytes: [[u8; DATE_VALUE_LENGTH]; 2],
    pos: [usize; 2],
    cnt: AtomicUsize,
}

impl Date {
    fn new() -> Date {
        let mut date = Date {
            bytes: [[0; DATE_VALUE_LENGTH], [0; DATE_VALUE_LENGTH]],
            pos: [0; 2],
            cnt: AtomicUsize::new(0),
        };
        date.update();
        date.cnt.store(1, Ordering::Relaxed);
        date.update();
        date
    }

    #[inline]
    fn as_bytes(&self) -> &[u8] {
        let id = self.cnt.load(Ordering::Relaxed) & 1;
        unsafe { self.bytes.get_unchecked(id) }
    }

    fn update(&mut self) {
        let id = self.cnt.load(Ordering::Relaxed) + 1;
        let idx = id & 1;
        self.pos[idx] = 0;
        write!(
            self,
            "{}",
            time::OffsetDateTime::now_utc().format("%a, %d %b %Y %H:%M:%S GMT")
        )
        .unwrap();
        self.cnt.store(id, Ordering::Relaxed);
    }
}

impl fmt::Write for Date {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let id = self.cnt.load(Ordering::Relaxed) + 1;
        let idx = id & 1;
        let len = s.len();
        self.bytes[idx][self.pos[idx]..self.pos[idx] + len].copy_from_slice(s.as_bytes());
        self.pos[idx] += len;
        Ok(())
    }
}
