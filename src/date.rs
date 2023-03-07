use std::cell::UnsafeCell;
use std::fmt::{self, Write};
use std::sync::Arc;

use bytes::BytesMut;
use once_cell::sync::Lazy;

// "Sun, 06 Nov 1994 08:49:37 GMT".len()
const DATE_VALUE_LENGTH: usize = 29;

static CURRENT_DATE: Lazy<Arc<DataWrap>> = Lazy::new(|| {
    let date = Arc::new(DataWrap(UnsafeCell::new(Date::new())));
    let date_clone = date.clone();
    may::go!(move || loop {
        may::coroutine::sleep(std::time::Duration::from_millis(500));
        unsafe { &mut *(date_clone.0).get() }.update();
    });
    date
});

struct DataWrap(UnsafeCell<Date>);
unsafe impl Sync for DataWrap {}

#[doc(hidden)]
#[inline]
pub fn append_date(dst: &mut BytesMut) {
    let date = unsafe { &*CURRENT_DATE.0.get() };
    dst.extend_from_slice(date.as_bytes());
}

struct Date {
    bytes: [u8; DATE_VALUE_LENGTH],
}

impl Date {
    fn new() -> Date {
        let mut date = Date {
            bytes: [0; DATE_VALUE_LENGTH],
        };
        date.update();
        date
    }

    #[inline]
    fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    fn update(&mut self) {
        let t = std::time::SystemTime::now();
        let date = httpdate::HttpDate::from(t);
        write!(self, "{date}").unwrap();
    }
}

impl fmt::Write for Date {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.bytes.copy_from_slice(s.as_bytes());
        Ok(())
    }
}
