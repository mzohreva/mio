use async_usercalls::{alloc_buf, CancelHandle, ReadBuffer, User, WriteBuffer};
use std::fmt;
use std::io::{self, IoSlice, IoSliceMut, Read, Write};
use std::mem;
use std::net::{self, Shutdown, SocketAddr};
use std::os::fortanix_sgx::io::AsRawFd;
use std::sync::{Arc, Mutex, MutexGuard};

use super::{other, would_block, State};
use crate::sys::sgx::selector::{EventKind, Provider, Registration};
use crate::{event, poll, Interest, Registry, Token};

const WRITE_BUFFER_SIZE: usize = 16 * 1024;
const READ_BUFFER_SIZE: usize = WRITE_BUFFER_SIZE;
const DEFAULT_FAKE_TTL: u32 = 64;

pub struct TcpStream {
    imp: StreamImp,
}

#[derive(Clone)]
struct StreamImp(Arc<Mutex<StreamInner>>);

struct StreamInner {
    connect_state: State<String, Option<CancelHandle>, net::TcpStream>,
    write_buffer: WriteBuffer,
    write_state: State<(), Option<CancelHandle>, ()>,
    read_buf: Option<User<[u8]>>,
    read_state: State<(), Option<CancelHandle>, ReadBuffer>,
    registration: Option<Registration>,
    provider: Option<Provider>,
}

impl TcpStream {
    fn new(connect_state: State<String, Option<CancelHandle>, net::TcpStream>) -> Self {
        TcpStream {
            imp: StreamImp(Arc::new(Mutex::new(StreamInner {
                connect_state,
                write_buffer: WriteBuffer::new(alloc_buf(WRITE_BUFFER_SIZE)),
                write_state: State::New(()),
                read_buf: Some(alloc_buf(READ_BUFFER_SIZE)),
                read_state: State::New(()),
                registration: None,
                provider: None,
            }))),
        }
    }

    pub(super) fn from_std(stream: net::TcpStream) -> (Self, SocketAddr) {
        let peer_addr = stream.peer_addr().unwrap_or_else(|_| ([0; 4], 0).into());
        let stream = TcpStream::new(State::Ready(stream));
        (stream, peer_addr)
    }

    pub fn connect(addr: SocketAddr) -> io::Result<TcpStream> {
        Ok(TcpStream::new(State::New(addr.to_string())))
    }

    pub fn connect_str(addr: &str) -> io::Result<TcpStream> {
        Ok(TcpStream::new(State::New(addr.to_owned())))
    }

    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.inner()
            .connect_state
            .as_ready()
            .ok_or_else(|| would_block())
            .and_then(|stream| stream.peer_addr())
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner()
            .connect_state
            .as_ready()
            .ok_or_else(|| would_block())
            .and_then(|stream| stream.local_addr())
    }

    pub fn shutdown(&self, _how: Shutdown) -> io::Result<()> {
        Ok(()) // ineffective in SGX
    }

    pub fn set_nodelay(&self, _nodelay: bool) -> io::Result<()> {
        Ok(()) // ineffective in SGX
    }

    pub fn nodelay(&self) -> io::Result<bool> {
        Ok(false) // ineffective in SGX
    }

    pub fn set_ttl(&self, _ttl: u32) -> io::Result<()> {
        Ok(()) // ineffective in SGX
    }

    pub fn ttl(&self) -> io::Result<u32> {
        Ok(DEFAULT_FAKE_TTL) // ineffective in SGX
    }

    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        let mut inner = self.inner();
        if let Some(err) = inner.connect_state.take_error(State::Error(io::ErrorKind::Other.into())) {
            return Ok(Some(err));
        }
        if let Some(err) = inner.read_state.take_error(State::New(())) {
            return Ok(Some(err));
        }
        if let Some(err) = inner.write_state.take_error(State::New(())) {
            return Ok(Some(err));
        }
        Ok(None)
    }

    pub fn peek(&self, _buf: &mut [u8]) -> io::Result<usize> {
        Ok(0) // undocumented current behavior in std::net::TcpStream for SGX target.
    }

    fn inner(&self) -> MutexGuard<'_, StreamInner> {
        self.imp.inner()
    }
}

impl StreamImp {
    fn inner(&self) -> MutexGuard<'_, StreamInner> {
        self.0.lock().unwrap()
    }

    fn schedule_connect_or_read(&self, inner: &mut StreamInner) {
        match inner.connect_state {
            State::New(_) => self.schedule_connect(inner),
            State::Ready(_) => self.post_connect(inner),
            State::Pending(_) | State::Error(_) => {},
        }
    }

    fn schedule_connect(&self, inner: &mut StreamInner) {
        let provider = match inner.provider.as_ref() {
            Some(provider) => provider,
            None => return,
        };
        let addr = match inner.connect_state {
            State::New(ref addr) => addr.as_str(),
            _ => return,
        };
        let weak_ref = Arc::downgrade(&self.0);
        let cancel_handle = provider.connect_stream(addr, move |res| {
            let imp = match weak_ref.upgrade() {
                Some(arc) => StreamImp(arc),
                None => return,
            };
            let mut inner = imp.inner();
            assert!(inner.connect_state.is_pending());
            inner.connect_state = res.into();
            imp.post_connect(&mut inner);
        });
        inner.connect_state = State::Pending(Some(cancel_handle));
    }

    fn post_connect(&self, inner: &mut StreamInner) {
        if inner.connect_state.is_ready() {
            inner.push_event(EventKind::Writable);
            self.schedule_read(inner);
        }
        if inner.connect_state.is_error() {
            inner.push_event(EventKind::WriteError);
        }
    }

    fn schedule_read(&self, inner: &mut StreamInner) {
        let provider = match inner.provider.as_ref() {
            Some(provider) => provider,
            None => return,
        };
        let fd = match (inner.read_state.is_new(), inner.connect_state.as_ready()) {
            (true, Some(stream)) => stream.as_raw_fd(),
            _ => return,
        };
        let read_buf = inner.read_buf.take().unwrap();
        let weak_ref = Arc::downgrade(&self.0);
        let cancel_handle = provider.read(fd, read_buf, move |res, read_buf| {
            let imp = match weak_ref.upgrade() {
                Some(arc) => StreamImp(arc),
                None => return,
            };
            let mut inner = imp.inner();
            assert!(inner.read_state.is_pending());
            match res {
                Ok(len) => {
                    inner.read_state = State::Ready(ReadBuffer::new(read_buf, len));
                    inner.push_event(if len == 0 {
                        EventKind::ReadClosed
                    } else {
                        EventKind::Readable
                    });
                }
                Err(e) => {
                    let is_closed = is_connection_closed(&e);
                    inner.read_state = State::Error(e);
                    inner.read_buf = Some(read_buf);
                    inner.push_event(if is_closed {
                        EventKind::ReadClosed
                    } else {
                        EventKind::ReadError
                    });
                }
            }
        });
        inner.read_state = State::Pending(Some(cancel_handle));
    }

    fn schedule_write(&self, inner: &mut StreamInner) {
        let provider = match inner.provider.as_ref() {
            Some(provider) => provider,
            None => return,
        };
        let fd = match (inner.write_state.is_new(), inner.connect_state.as_ready()) {
            (true, Some(stream)) => stream.as_raw_fd(),
            _ => return,
        };
        let chunk = match inner.write_buffer.consumable_chunk() {
            Some(chunk) => chunk,
            None => return,
        };
        let imp = self.clone();
        let cancel_handle = provider.write(fd, chunk, move |res, buf| {
            let mut inner = imp.inner();
            match res {
                Ok(0) => {
                    // since we don't write 0 bytes, this signifies EOF
                    inner.write_state = State::Error(io::ErrorKind::WriteZero.into());
                    inner.push_event(EventKind::WriteClosed);
                }
                Ok(n) => {
                    inner.write_buffer.consume(buf, n);
                    inner.write_state = State::New(());
                    if !inner.write_buffer.is_empty() {
                        imp.schedule_write(&mut inner);
                    } else {
                        inner.push_event(EventKind::Writable);
                    }
                }
                Err(e) => {
                    let is_closed = is_connection_closed(&e);
                    inner.write_state = State::Error(e);
                    inner.push_event(if is_closed {
                        EventKind::WriteClosed
                    } else {
                        EventKind::WriteError
                    });
                }
            }
        });
        inner.write_state = State::Pending(Some(cancel_handle));
    }

    fn read_vectored(&self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        let mut inner = self.inner();
        let ret = match mem::replace(&mut inner.read_state, State::New(())) {
            State::New(()) => Err(would_block()),
            State::Pending(cancel_handle) => {
                inner.read_state = State::Pending(cancel_handle);
                return Err(would_block());
            }
            State::Ready(mut read_buf) => {
                let mut r = 0;
                for buf in bufs {
                    r += read_buf.read(buf);
                }
                match read_buf.remaining_bytes() {
                    // Only schedule another read if the previous one returned some bytes.
                    // Otherwise assume subsequent reads will always return 0 bytes, so just
                    // stay at Ready state and always return 0 bytes from this point on.
                    0 if read_buf.len() > 0 => inner.read_buf = Some(read_buf.into_inner()),
                    _ => inner.read_state = State::Ready(read_buf),
                }
                Ok(r)
            }
            State::Error(e) => Err(e),
        };
        self.schedule_read(&mut inner);
        ret
    }

    fn write_vectored(&self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        let mut inner = self.inner();
        if let Some(e) = inner.write_state.take_error(State::New(())) {
            return Err(e);
        }
        if !inner.connect_state.is_ready() {
            return Err(would_block());
        }
        let written = inner.write_buffer.write_vectored(bufs);
        if written == 0 {
            return Err(would_block());
        }
        self.schedule_write(&mut inner);
        Ok(written)
    }
}

impl StreamInner {
    fn push_event(&self, kind: EventKind) {
        if let Some(ref registration) = self.registration {
            registration.push_event(kind);
        }
    }

    fn announce_current_state(&self) {
        if self.connect_state.is_ready() {
            self.push_event(EventKind::Writable);
        }
        if self.connect_state.is_error() {
            self.push_event(EventKind::WriteError);
        }
        if self.read_state.is_ready() {
            self.push_event(EventKind::Readable);
        }
        if self.read_state.is_error() {
            self.push_event(EventKind::ReadError);
        }
    }
}

impl From<net::TcpStream> for TcpStream {
    fn from(stream: net::TcpStream) -> Self {
        TcpStream::new(State::Ready(stream))
    }
}

impl event::Source for TcpStream {
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
        self.imp.schedule_connect_or_read(&mut inner);
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
        if changed {
            inner.announce_current_state();
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

impl fmt::Debug for TcpStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let inner = self.inner();
        let mut res = f.debug_struct("TcpStream");
        res.field("connect_state", &inner.connect_state);
        res.field("read_state", &inner.read_state);
        res.field("write_state", &inner.write_state);
        res.finish()
    }
}

impl Read for TcpStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.imp.read_vectored(&mut [IoSliceMut::new(buf)])
    }

    fn read_vectored(&mut self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        self.imp.read_vectored(bufs)
    }
}

impl<'a> Read for &'a TcpStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.imp.read_vectored(&mut [IoSliceMut::new(buf)])
    }

    fn read_vectored(&mut self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        self.imp.read_vectored(bufs)
    }
}

impl Write for TcpStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.imp.write_vectored(&[IoSlice::new(buf)])
    }

    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        self.imp.write_vectored(bufs)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(()) // same as in `impl Write for std::net::TcpStream`
    }
}

impl<'a> Write for &'a TcpStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.imp.write_vectored(&[IoSlice::new(buf)])
    }

    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        self.imp.write_vectored(bufs)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(()) // same as in `impl Write for std::net::TcpStream`
    }
}

impl Drop for TcpStream {
    fn drop(&mut self) {
        let mut inner = self.inner();
        // deregister so we don't send events after drop
        inner.registration = None;
        if let Some(cancel_handle) = inner.connect_state.as_pending_mut().and_then(|opt| opt.take()) {
            cancel_handle.cancel();
        }
        if let Some(cancel_handle) = inner.read_state.as_pending_mut().and_then(|opt| opt.take()) {
            cancel_handle.cancel();
        }
        // NOTE: We don't cancel write since we have promised to write those bytes before drop.
        // Also note that the callback in schedule_write() holds an Arc not a Weak, so it can
        // continue writing the remaining bytes in the write buffer.
    }
}

fn is_connection_closed(e: &io::Error) -> bool {
    match e.kind() {
        io::ErrorKind::ConnectionReset
        | io::ErrorKind::ConnectionAborted
        | io::ErrorKind::BrokenPipe => true,
        _ => false,
    }
}
