use crate::sys::sgx::selector::{EventKind, Registration};
use crate::sys::Selector;
use crate::{Interest, Token};
use std::fmt;
use std::io;

pub struct Waker(Registration);

impl Waker {
    pub fn new(selector: &Selector, token: Token) -> io::Result<Waker> {
        Ok(Waker(Registration::new(
            selector,
            token,
            Interest::READABLE,
        )))
    }

    pub fn wake(&self) -> io::Result<()> {
        self.0.push_event(EventKind::Readable);
        Ok(())
    }
}

impl fmt::Debug for Waker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Waker")
            .field("token", &self.0.token())
            .finish()
    }
}
