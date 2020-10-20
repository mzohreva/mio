use async_usercalls::CancelHandle;
use std::fmt;
use std::io;
use std::mem;
use std::net::{self, SocketAddr};
use std::os::fortanix_sgx::io::AsRawFd;
use std::os::fortanix_sgx::usercalls::raw::Fd;
use std::sync::{Arc, Mutex, MutexGuard};

use super::{other, would_block, State, TcpStream};
use crate::sys::sgx::selector::{EventKind, Provider, Registration};
use crate::{event, poll, Interest, Registry, Token};

pub struct TcpListener {
    listener: net::TcpListener,
    imp: ListenerImp,
}

#[derive(Clone)]
struct ListenerImp(Arc<Mutex<ListenerInner>>);

struct ListenerInner {
    fd: Fd,
    accept_state: State<(), Option<CancelHandle>, net::TcpStream>,
    registration: Option<Registration>,
    provider: Option<Provider>,
}

impl TcpListener {
    fn from_std(listener: net::TcpListener) -> TcpListener {
        TcpListener {
            imp: ListenerImp(Arc::new(Mutex::new(ListenerInner {
                fd: listener.as_raw_fd(),
                accept_state: State::New(()),
                registration: None,
                provider: None,
            }))),
            listener,
        }
    }

    pub fn bind(addr: SocketAddr) -> io::Result<TcpListener> {
        Ok(TcpListener::from_std(net::TcpListener::bind(addr)?))
    }

    pub fn bind_str(addr: &str) -> io::Result<TcpListener> {
        Ok(TcpListener::from_std(net::TcpListener::bind(addr)?))
    }

    pub fn accept(&self) -> io::Result<(TcpStream, SocketAddr)> {
        let mut inner = self.inner();
        let ret = match mem::replace(&mut inner.accept_state, State::New(())) {
            State::New(()) => Err(would_block()),
            State::Pending(cancel_handle) => {
                inner.accept_state = State::Pending(cancel_handle);
                return Err(would_block());
            }
            State::Ready(stream) => Ok(TcpStream::from_std(stream)),
            State::Error(e) => Err(e),
        };
        self.imp.schedule_accept(&mut inner);
        ret
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    pub fn set_ttl(&self, ttl: u32) -> io::Result<()> {
        self.listener.set_ttl(ttl)
    }

    pub fn ttl(&self) -> io::Result<u32> {
        self.listener.ttl()
    }

    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        self.listener.take_error()
    }

    fn inner(&self) -> MutexGuard<'_, ListenerInner> {
        self.imp.inner()
    }
}

impl ListenerImp {
    fn inner(&self) -> MutexGuard<'_, ListenerInner> {
        self.0.lock().unwrap()
    }

    fn schedule_accept(&self, inner: &mut ListenerInner) {
        let provider = match inner.provider.as_ref() {
            Some(provider) => provider,
            None => return,
        };
        match inner.accept_state {
            State::New(()) => {}
            _ => return,
        }
        let weak_ref = Arc::downgrade(&self.0);
        let cancel_handle = provider.accept_stream(inner.fd, move |res| {
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

impl From<net::TcpListener> for TcpListener {
    fn from(listener: net::TcpListener) -> Self {
        TcpListener::from_std(listener)
    }
}

impl event::Source for TcpListener {
    fn register(
        &mut self,
        registry: &Registry,
        token: Token,
        interest: Interest,
    ) -> io::Result<()> {
        let mut inner = self.inner();
        match inner.registration {
            Some(_) => return Err(other("I/O source already registered with a `Registry`")),
            None => inner.registration = Some(Registration::new(poll::selector(registry), token, interest)),
        }
        inner.provider = Some(Provider::new(poll::selector(registry)));
        self.imp.schedule_accept(&mut inner);
        Ok(())
    }

    fn reregister(
        &mut self,
        _registry: &Registry,
        token: Token,
        interest: Interest,
    ) -> io::Result<()> {
        let mut inner = self.inner();
        let changed = match inner.registration {
            Some(ref mut registration) => registration.change_details(token, interest),
            None => return Err(other("I/O source not registered with `Registry`")),
        };
        if changed && inner.accept_state.is_ready() {
            inner.push_event(EventKind::Readable);
        }
        if changed && inner.accept_state.is_error() {
            inner.push_event(EventKind::ReadError);
        }
        Ok(())
    }

    fn deregister(&mut self, _registry: &Registry) -> io::Result<()> {
        let mut inner = self.inner();
        match inner.registration {
            Some(_) => inner.registration = None,
            None => return Err(other("I/O source not registered with `Registry`")),
        }
        Ok(())
    }
}

impl fmt::Debug for TcpListener {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let inner = self.inner();
        let mut res = f.debug_struct("TcpListener");
        res.field("accept_state", &inner.accept_state);
        res.field("listener", &self.listener);
        res.finish()
    }
}

impl Drop for TcpListener {
    fn drop(&mut self) {
        let mut inner = self.inner();
        // deregister so we don't send events after drop
        inner.registration = None;
        if let Some(cancel_handle) = inner.accept_state.as_pending_mut().and_then(|opt| opt.take()) {
            cancel_handle.cancel();
        }
    }
}
