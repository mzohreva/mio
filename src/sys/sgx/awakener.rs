use event::Evented;
use poll::selector;
use std::fmt;
use std::sync::Mutex;
use sys::sgx::selector::{EventKind, Registration};
use sys::sgx::{check_opts, other};
use {io, Poll, PollOpt, Ready, Token};

pub struct Awakener(Mutex<Option<Registration>>);

impl Awakener {
    pub fn new() -> io::Result<Awakener> {
        Ok(Awakener(Mutex::new(None)))
    }

    pub fn wakeup(&self) -> io::Result<()> {
        match self.0.lock().unwrap().as_ref() {
            Some(reg) => reg.push_event(EventKind::Readable),
            None => {} // doing nothing here seems ok.
        }
        Ok(())
    }

    pub fn cleanup(&self) {}
}

impl Evented for Awakener {
    fn register(
        &self,
        poll: &Poll,
        token: Token,
        interest: Ready,
        opts: PollOpt,
    ) -> io::Result<()> {
        check_opts(opts)?;
        let mut reg = self.0.lock().unwrap();
        match reg.as_ref() {
            Some(_) => Err(other("awakener already registered")),
            None => {
                *reg = Some(Registration::new(selector(poll), token, interest));
                Ok(())
            }
        }
    }

    fn reregister(
        &self,
        _poll: &Poll,
        token: Token,
        interest: Ready,
        opts: PollOpt,
    ) -> io::Result<()> {
        check_opts(opts)?;
        match self.0.lock().unwrap().as_mut() {
            Some(reg) => {
                reg.change_details(token, interest);
                Ok(())
            }
            None => Err(other("awakener have not been registered previously")),
        }
    }

    fn deregister(&self, _poll: &Poll) -> io::Result<()> {
        let reg = self.0.lock().unwrap().take();
        match reg {
            Some(_) => Ok(()),
            None => Err(other("awakener have not been registered previously")),
        }
    }
}

impl fmt::Debug for Awakener {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Awakener").finish()
    }
}
