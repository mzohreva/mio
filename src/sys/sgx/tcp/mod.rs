use async_usercalls::{ReadBuffer, WriteBuffer};
use std::fmt;
use std::io;
use std::mem;
use std::net::SocketAddr;
use std::ops::{Deref, DerefMut};
use std::os::fortanix_sgx::usercalls::alloc::User;

mod listener;
mod stream;

pub use self::listener::TcpListener;
pub use self::stream::TcpStream;

pub fn connect(addr: SocketAddr) -> io::Result<TcpStream> {
    TcpStream::connect(addr)
}

pub fn connect_str(addr: &str) -> io::Result<TcpStream> {
    TcpStream::connect_str(addr)
}

pub fn bind(addr: SocketAddr) -> io::Result<TcpListener> {
    TcpListener::bind(addr)
}

pub fn bind_str(addr: &str) -> io::Result<TcpListener> {
    TcpListener::bind_str(addr)
}

pub fn accept(listener: &TcpListener) -> io::Result<(TcpStream, SocketAddr)> {
    listener.accept()
}

enum State<N, P, R> {
    New(N),
    Pending(P),
    Ready(R),
    Error(io::Error),
}

impl<N, P, R> State<N, P, R> {
    fn as_ready(&self) -> Option<&R> {
        match self {
            State::Ready(ref r) => Some(r),
            _ => None,
        }
    }

    fn as_pending_mut(&mut self) -> Option<&mut P> {
        match self {
            State::Pending(ref mut p) => Some(p),
            _ => None,
        }
    }

    fn is_new(&self) -> bool {
        match self {
            State::New(_) => true,
            _ => false,
        }
    }

    fn is_pending(&self) -> bool {
        match self {
            State::Pending(_) => true,
            _ => false,
        }
    }

    fn is_ready(&self) -> bool {
        match self {
            State::Ready(_) => true,
            _ => false,
        }
    }

    fn is_error(&self) -> bool {
        match self {
            State::Error(_) => true,
            _ => false,
        }
    }

    fn take_error(&mut self, replacement: State<N, P, R>) -> Option<io::Error> {
        if self.is_error() {
            match mem::replace(self, replacement) {
                State::Error(e) => return Some(e),
                _ => unreachable!(),
            }
        }
        None
    }
}

impl<N, P, R> From<io::Result<R>> for State<N, P, R> {
    fn from(res: io::Result<R>) -> Self {
        match res {
            Ok(r) => State::Ready(r),
            Err(e) => State::Error(e),
        }
    }
}

impl<N, P, R> fmt::Debug for State<N, P, R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            State::New(_) => f.pad("new"),
            State::Pending(_) => f.pad("pending"),
            State::Ready(_) => f.pad("ready"),
            State::Error(_) => f.pad("error"),
        }
    }
}

fn other(s: &str) -> io::Error {
    io::Error::new(io::ErrorKind::Other, s)
}

fn would_block() -> io::Error {
    io::ErrorKind::WouldBlock.into()
}

// Interim solution until we mark the target types appropriately
pub(crate) struct MakeSend<T>(T);

impl<T> MakeSend<T> {
    pub fn new(t: T) -> Self {
        Self(t)
    }

    #[allow(unused)]
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> Deref for MakeSend<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for MakeSend<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

unsafe impl Send for MakeSend<User<[u8]>> {}
unsafe impl Send for MakeSend<ReadBuffer> {}
unsafe impl Send for MakeSend<WriteBuffer> {}
