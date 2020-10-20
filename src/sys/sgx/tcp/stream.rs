use async_usercalls::{ReadBuffer, WriteBuffer, CancelHandle};
use event::Evented;
use iovec::IoVec;
use poll::selector;
use std::fmt;
use std::io::{Read, Write};
use std::mem;
use std::net::{self, Shutdown, SocketAddr};
use std::os::fortanix_sgx::io::AsRawFd;
use std::os::fortanix_sgx::usercalls::alloc::User;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use sys::sgx::selector::{EventKind, Provider, Registration};
use sys::sgx::tcp::{MakeSend, State};
use sys::sgx::{check_opts, other, would_block};
use {io, Poll, PollOpt, Ready, Token};

const WRITE_BUFFER_SIZE: usize = 16 * 1024;
const READ_BUFFER_SIZE: usize = WRITE_BUFFER_SIZE;
const DEFAULT_FAKE_TTL: u32 = 64;

#[derive(Clone)]
pub struct TcpStream(Arc<TcpStreamInner>);

struct TcpStreamInner {
    imp: StreamImp,
}

#[derive(Clone)]
struct StreamImp(Arc<Mutex<StreamInner>>);

struct StreamInner {
    connect_state: State<String, Option<CancelHandle>, net::TcpStream>,
    write_buffer: MakeSend<WriteBuffer>,
    write_state: State<(), Option<CancelHandle>, ()>,
    read_buf: Option<MakeSend<User<[u8]>>>,
    read_state: State<(), Option<CancelHandle>, MakeSend<ReadBuffer>>,
    registration: Option<Registration>,
    provider: Option<Provider>,
}

impl TcpStream {
    fn new(connect_state: State<String, Option<CancelHandle>, net::TcpStream>) -> Self {
        TcpStream(Arc::new(TcpStreamInner {
            imp: StreamImp(Arc::new(Mutex::new(StreamInner {
                connect_state,
                write_buffer: MakeSend::new(WriteBuffer::new(User::<[u8]>::uninitialized(WRITE_BUFFER_SIZE))),
                write_state: State::New(()),
                read_buf: Some(MakeSend::new(User::<[u8]>::uninitialized(READ_BUFFER_SIZE))),
                read_state: State::New(()),
                registration: None,
                provider: None,
            }))),
        }))
    }

    pub fn connect(addr: &SocketAddr) -> io::Result<TcpStream> {
        Ok(TcpStream::new(State::New(addr.to_string())))
    }

    pub fn connect_str(addr: &str) -> io::Result<TcpStream> {
        Ok(TcpStream::new(State::New(addr.to_owned())))
    }

    pub fn from_stream(stream: net::TcpStream) -> TcpStream {
        TcpStream::new(State::Ready(stream))
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

    pub fn try_clone(&self) -> io::Result<TcpStream> {
        Ok(self.clone())
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

    pub fn set_recv_buffer_size(&self, _size: usize) -> io::Result<()> {
        Ok(()) // ineffective in SGX
    }

    pub fn recv_buffer_size(&self) -> io::Result<usize> {
        Ok(WRITE_BUFFER_SIZE) // ineffective in SGX
    }

    pub fn set_send_buffer_size(&self, _size: usize) -> io::Result<()> {
        Ok(()) // ineffective in SGX
    }

    pub fn send_buffer_size(&self) -> io::Result<usize> {
        Ok(WRITE_BUFFER_SIZE) // ineffective in SGX
    }

    pub fn set_keepalive(&self, _keepalive: Option<Duration>) -> io::Result<()> {
        Ok(()) // ineffective in SGX
    }

    pub fn keepalive(&self) -> io::Result<Option<Duration>> {
        Ok(None) // ineffective in SGX
    }

    pub fn set_ttl(&self, _ttl: u32) -> io::Result<()> {
        Ok(()) // ineffective in SGX
    }

    pub fn ttl(&self) -> io::Result<u32> {
        Ok(DEFAULT_FAKE_TTL) // ineffective in SGX
    }

    pub fn set_only_v6(&self, _only_v6: bool) -> io::Result<()> {
        Ok(()) // ineffective in SGX
    }

    pub fn only_v6(&self) -> io::Result<bool> {
        Ok(false) // ineffective in SGX
    }

    pub fn set_linger(&self, _dur: Option<Duration>) -> io::Result<()> {
        Ok(()) // ineffective in SGX
    }

    pub fn linger(&self) -> io::Result<Option<Duration>> {
        Ok(None) // ineffective in SGX
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

    pub fn readv(&self, bufs: &mut [&mut IoVec]) -> io::Result<usize> {
        self.0.imp.read_vectored(bufs)
    }

    pub fn writev(&self, bufs: &[&IoVec]) -> io::Result<usize> {
        self.0.imp.write_vectored(bufs)
    }

    fn inner(&self) -> MutexGuard<'_, StreamInner> {
        self.0.imp.inner()
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
        let read_buf = inner.read_buf.take().unwrap().into_inner();
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
                    inner.read_state = State::Ready(MakeSend::new(ReadBuffer::new(read_buf, len)));
                    inner.push_event(if len == 0 {
                        EventKind::ReadClosed
                    } else {
                        EventKind::Readable
                    });
                }
                Err(e) => {
                    let is_closed = is_connection_closed(&e);
                    inner.read_state = State::Error(e);
                    inner.read_buf = Some(MakeSend::new(read_buf));
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

    fn read_vectored(&self, bufs: &mut [&mut IoVec]) -> io::Result<usize> {
        let mut inner = self.inner();
        let ret = match mem::replace(&mut inner.read_state, State::New(())) {
            State::New(()) => Err(would_block()),
            State::Pending(cancel_handle) => {
                inner.read_state = State::Pending(cancel_handle);
                return Err(would_block());
            }
            State::Ready(read_buf) => {
                let mut read_buf = read_buf.into_inner();
                let mut r = 0;
                for buf in bufs {
                    r += read_buf.read(buf);
                }
                match read_buf.remaining_bytes() {
                    // Only schedule another read if the previous one returned some bytes.
                    // Otherwise assume subsequent reads will always return 0 bytes, so just
                    // stay at Ready state and always return 0 bytes from this point on.
                    0 if read_buf.len() > 0 => inner.read_buf = Some(MakeSend::new(read_buf.into_inner())),
                    _ => inner.read_state = State::Ready(MakeSend::new(read_buf)),
                }
                Ok(r)
            }
            State::Error(e) => Err(e),
        };
        self.schedule_read(&mut inner);
        ret
    }

    fn write_vectored(&self, bufs: &[&IoVec]) -> io::Result<usize> {
        let mut inner = self.inner();
        if let Some(e) = inner.write_state.take_error(State::New(())) {
            return Err(e);
        }
        if !inner.connect_state.is_ready() {
            return Err(would_block());
        }
        let mut written = 0;
        for buf in bufs {
            written += inner.write_buffer.write(buf);
        }
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

impl<'a> Read for &'a TcpStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if let Some(io_vec) = IoVec::from_bytes_mut(buf) {
            return self.0.imp.read_vectored(&mut [io_vec]);
        }
        Ok(0)
    }
}

impl<'a> Write for &'a TcpStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if let Some(io_vec) = IoVec::from_bytes(buf) {
            return self.0.imp.write_vectored(&[io_vec]);
        }
        Ok(0)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(()) // same as in `impl Write for std::net::TcpStream`
    }
}

impl Evented for TcpStream {
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
        inner.provider = Some(Provider::new(selector(poll)));
        self.0.imp.schedule_connect_or_read(&mut inner);
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
        if changed {
            inner.announce_current_state();
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

impl Drop for TcpStreamInner {
    fn drop(&mut self) {
        let mut inner = self.imp.inner();
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
