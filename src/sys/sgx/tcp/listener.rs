use event::Evented;
use poll::selector;
use std::fmt;
use std::mem;
use std::net::{self, SocketAddr};
use std::os::fortanix_sgx::io::AsRawFd;
use std::os::fortanix_sgx::usercalls::raw::Fd;
use std::sync::{Arc, Mutex, MutexGuard};
use sys::sgx::selector::{EventKind, Registration};
use sys::sgx::tcp::{CancelHandleOpt, State, ASYNC};
use sys::sgx::{check_opts, other, would_block};
use {io, Poll, PollOpt, Ready, Token};

#[derive(Clone)]
pub struct TcpListener(Arc<TcpListenerInner>);

struct TcpListenerInner {
    listener: net::TcpListener,
    imp: ListenerImp,
}

#[derive(Clone)]
struct ListenerImp(Arc<Mutex<ListenerInner>>);

struct ListenerInner {
    fd: Fd,
    accept_state: State<(), CancelHandleOpt, net::TcpStream>,
    registration: Option<Registration>,
}

impl TcpListener {
    pub fn new(inner: net::TcpListener) -> io::Result<TcpListener> {
        let fd = inner.as_raw_fd();
        Ok(TcpListener(Arc::new(TcpListenerInner {
            listener: inner,
            imp: ListenerImp(Arc::new(Mutex::new(ListenerInner {
                fd,
                accept_state: State::New(()),
                registration: None,
            }))),
        })))
    }

    pub fn bind(addr: &SocketAddr) -> io::Result<TcpListener> {
        let listener = net::TcpListener::bind(addr)?;
        TcpListener::new(listener)
    }

    pub fn bind_str(addr: &str) -> io::Result<TcpListener> {
        let listener = net::TcpListener::bind(addr)?;
        TcpListener::new(listener)
    }

    pub fn try_clone(&self) -> io::Result<TcpListener> {
        Ok(self.clone())
    }

    pub fn accept(&self) -> io::Result<(net::TcpStream, SocketAddr)> {
        let mut inner = self.inner();
        let ret = match mem::replace(&mut inner.accept_state, State::New(())) {
            State::New(()) => Err(would_block()),
            State::Pending(cancel_handle) => {
                inner.accept_state = State::Pending(cancel_handle);
                return Err(would_block());
            }
            State::Ready(stream) => {
                let peer_addr = stream.peer_addr().unwrap_or_else(|_| ([0; 4], 0).into());
                Ok((stream, peer_addr))
            }
            State::Error(e) => Err(e),
        };
        self.0.imp.schedule_accept(&mut inner);
        ret
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.0.listener.local_addr()
    }

    #[allow(deprecated)]
    pub fn set_only_v6(&self, only_v6: bool) -> io::Result<()> {
        self.0.listener.set_only_v6(only_v6)
    }

    #[allow(deprecated)]
    pub fn only_v6(&self) -> io::Result<bool> {
        self.0.listener.only_v6()
    }

    pub fn set_ttl(&self, ttl: u32) -> io::Result<()> {
        self.0.listener.set_ttl(ttl)
    }

    pub fn ttl(&self) -> io::Result<u32> {
        self.0.listener.ttl()
    }

    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        self.0.listener.take_error()
    }

    fn inner(&self) -> MutexGuard<'_, ListenerInner> {
        self.0.imp.inner()
    }
}

impl ListenerImp {
    fn inner(&self) -> MutexGuard<'_, ListenerInner> {
        self.0.lock().unwrap()
    }

    fn schedule_accept(&self, inner: &mut ListenerInner) {
        match inner.accept_state {
            State::New(()) => {}
            _ => return,
        }
        let weak_ref = Arc::downgrade(&self.0);
        let cancel_handle = ASYNC.accept_stream(inner.fd, move |res| {
            let imp = match weak_ref.upgrade() {
                Some(arc) => ListenerImp(arc),
                None => return,
            };
            let mut inner = imp.inner();
            assert!(inner.accept_state.is_pending());
            inner.accept_state = res.into();
            inner.push_event(if inner.accept_state.is_error() {
                EventKind::ReadError
            } else {
                EventKind::Readable
            });
        });
        inner.accept_state = State::Pending(Some(cancel_handle));
    }
}

impl ListenerInner {
    fn push_event(&self, kind: EventKind) {
        if let Some(ref registration) = self.registration {
            registration.push_event(kind);
        }
    }
}

impl Evented for TcpListener {
    fn register(
        &self,
        poll: &Poll,
        token: Token,
        interest: Ready,
        opts: PollOpt,
    ) -> io::Result<()> {
        check_opts(opts)?;
        let mut inner = self.inner();
        match inner.registration {
            Some(_) => return Err(other("I/O source already registered")),
            None => inner.registration = Some(Registration::new(selector(poll), token, interest)),
        }
        self.0.imp.schedule_accept(&mut inner);
        Ok(())
    }

    fn reregister(
        &self,
        _poll: &Poll,
        token: Token,
        interest: Ready,
        opts: PollOpt,
    ) -> io::Result<()> {
        check_opts(opts)?;
        let mut inner = self.inner();
        let changed = match inner.registration {
            Some(ref mut registration) => registration.change_details(token, interest),
            None => return Err(other("I/O source not registered")),
        };
        if changed && inner.accept_state.is_ready() {
            inner.push_event(EventKind::Readable);
        }
        if changed && inner.accept_state.is_error() {
            inner.push_event(EventKind::ReadError);
        }
        Ok(())
    }

    fn deregister(&self, _poll: &Poll) -> io::Result<()> {
        let mut inner = self.inner();
        match inner.registration {
            Some(_) => inner.registration = None,
            None => return Err(other("I/O source not registered")),
        }
        Ok(())
    }
}

impl fmt::Debug for TcpListener {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let inner = self.inner();
        let mut res = f.debug_struct("TcpListener");
        res.field("accept_state", &inner.accept_state);
        res.field("listener", &self.0.listener);
        res.finish()
    }
}

impl Drop for TcpListenerInner {
    fn drop(&mut self) {
        let mut inner = self.imp.inner();
        // deregister so we don't send events after drop
        inner.registration = None;
        if let Some(cancel_handle) = inner.accept_state.as_pending_mut().and_then(|opt| opt.take()) {
            cancel_handle.cancel();
        }
    }
}
