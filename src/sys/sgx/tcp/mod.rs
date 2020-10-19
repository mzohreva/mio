use std::fmt;
use std::io;
use std::mem;

mod listener;
mod stream;

pub use self::listener::TcpListener;
pub use self::stream::TcpStream;

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
