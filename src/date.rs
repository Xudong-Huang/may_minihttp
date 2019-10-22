use std::fmt::{self, Write};
use std::str;
use std::sync::Arc;

use bytes::{BufMut, BytesMut};
use lazy_static::lazy_static;
use rcu_cell::RcuCell;

// "Sun, 06 Nov 1994 08:49:37 GMT".len()
const DATE_VALUE_LENGTH: usize = 29;

lazy_static! {
    static ref CURRENT_DATE: Arc<RcuCell<Date>> = {
        let date = Arc::new(RcuCell::new(Some(Date::new())));
        let date_clone = date.clone();
        may::go!(move || loop {
            may::coroutine::sleep(std::time::Duration::from_millis(500));
            date_clone.try_lock().unwrap().update(Some(Date::new()));
        });
        date
    };
}

#[doc(hidden)]
pub fn set_date(dst: &mut BytesMut) {
    dst.put_slice(CURRENT_DATE.read().unwrap().as_bytes());
}

struct Date {
    bytes: [u8; DATE_VALUE_LENGTH],
    pos: usize,
}

impl Date {
    fn new() -> Date {
        let mut date = Date {
            bytes: [0; DATE_VALUE_LENGTH],
            pos: 0,
        };
        date.update();
        date
    }

    #[inline]
    fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    fn update(&mut self) {
        self.pos = 0;
        write!(self, "{}", time::at_utc(time::get_time()).rfc822()).unwrap();
    }
}

impl fmt::Write for Date {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let len = s.len();
        self.bytes[self.pos..self.pos + len].copy_from_slice(s.as_bytes());
        self.pos += len;
        Ok(())
    }
}
