use async_usercalls::{AsyncUsercallProvider, CallbackHandler, CallbackHandlerWaker};
use crossbeam_channel as mpmc;
use std::collections::HashMap;
use std::io;
use std::ops::Deref;
use std::os::fortanix_sgx::io::{AsRawFd, RawFd};
#[cfg(debug_assertions)]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub struct Selector {
    #[cfg(debug_assertions)]
    id: usize,
    event_rx: mpmc::Receiver<(RegistrationId, EventKind)>,
    callback_handler: Arc<CallbackHandler>,
    shared_inner: Arc<SelectorSharedInner>,
    #[cfg(debug_assertions)]
    has_waker: AtomicBool,
}

struct SelectorSharedInner {
    event_tx: mpmc::Sender<(RegistrationId, EventKind)>,
    registrations: Mutex<HashMap<RegistrationId, (Token, Interest)>>,
    provider: AsyncUsercallProvider,
    callback_handler_waker: CallbackHandlerWaker,
}

impl Selector {
    pub fn new() -> io::Result<Selector> {
        #[cfg(debug_assertions)]
        static NEXT_ID: AtomicUsize = AtomicUsize::new(1);
        let (event_tx, event_rx) = mpmc::unbounded();
        let (provider, callback_handler) = AsyncUsercallProvider::new();
        let callback_handler_waker = callback_handler.waker();
        Ok(Selector {
            #[cfg(debug_assertions)]
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            event_rx,
            callback_handler: Arc::new(callback_handler),
            shared_inner: Arc::new(SelectorSharedInner {
                event_tx,
                registrations: Mutex::new(HashMap::new()),
                provider,
                callback_handler_waker,
            }),
            #[cfg(debug_assertions)]
            has_waker: AtomicBool::new(false),
        })
    }

    pub fn try_clone(&self) -> io::Result<Selector> {
        Ok(Selector {
            #[cfg(debug_assertions)]
            id: self.id,
            event_rx: self.event_rx.clone(),
            callback_handler: self.callback_handler.clone(),
            shared_inner: self.shared_inner.clone(),
            #[cfg(debug_assertions)]
            has_waker: AtomicBool::new(self.has_waker.load(Ordering::Acquire)),
        })
    }

    pub fn select(&self, events: &mut Events, mut timeout: Option<Duration>) -> io::Result<()> {
        self.shared_inner.callback_handler_waker.clear();
        if !self.event_rx.is_empty() {
            timeout = Some(Duration::from_nanos(0));
        }
        self.callback_handler.poll(timeout);

        events.clear();
        let registrations = self.shared_inner.registrations.lock().unwrap();
        for (reg_id, kind) in self.event_rx.try_iter() {
            if let Some((token, interest)) = registrations.get(&reg_id) {
                if kind.matches_interest(interest) {
                    events.push(Event::new(kind, *token));
                }
            }
            if events.len() == events.capacity() {
                break;
            }
        }
        Ok(())
    }

    #[cfg(debug_assertions)]
    pub fn register_waker(&self) -> bool {
        self.has_waker.swap(true, Ordering::AcqRel)
    }
}

cfg_io_source! {
    use crate::{Interest, Token};

    impl Selector {
        pub fn register(&self, _: RawFd, _: Token, _: Interest) -> io::Result<()> {
            unimplemented!();
        }

        pub fn reregister(&self, _: RawFd, _: Token, _: Interest) -> io::Result<()> {
            unimplemented!();
        }

        pub fn deregister(&self, _: RawFd) -> io::Result<()> {
            unimplemented!();
        }
    }
}

cfg_net! {
    #[cfg(debug_assertions)]
    impl Selector {
        pub fn id(&self) -> usize {
            self.id
        }
    }
}

impl AsRawFd for Selector {
    fn as_raw_fd(&self) -> RawFd {
        unimplemented!()
    }
}

pub(crate) struct Provider(Arc<SelectorSharedInner>);

impl Provider {
    pub fn new(selector: &Selector) -> Self {
        Self(selector.shared_inner.clone())
    }
}

impl Deref for Provider {
    type Target = AsyncUsercallProvider;

    fn deref(&self) -> &Self::Target {
        &self.0.provider
    }
}

pub(crate) struct Registration {
    id: RegistrationId,
    shared_inner: Arc<SelectorSharedInner>,
    token: Token,
    interest: Interest,
}

impl Registration {
    pub fn new(selector: &Selector, token: Token, interest: Interest) -> Self {
        let id = RegistrationId::new();
        selector.shared_inner.registrations.lock().unwrap().insert(id, (token, interest));
        Registration {
            id,
            shared_inner: selector.shared_inner.clone(),
            token,
            interest: interest,
        }
    }

    pub fn provider(&self) -> Provider {
        Provider(self.shared_inner.clone())
    }

    pub fn change_details(&mut self, token: Token, interest: Interest) -> bool {
        if self.token == token && self.interest == interest {
            return false;
        }
        self.token = token;
        self.interest = interest;
        self.shared_inner.registrations.lock().unwrap().insert(self.id, (self.token, self.interest));
        true
    }

    pub fn token(&self) -> Token {
        self.token
    }

    pub fn push_event(&self, kind: EventKind) {
        if kind.matches_interest(&self.interest) {
            let _ = self.shared_inner.event_tx.send((self.id, kind));
            self.shared_inner.callback_handler_waker.wake();
        }
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        self.shared_inner.registrations.lock().unwrap().remove(&self.id);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct RegistrationId(usize);

impl RegistrationId {
    fn new() -> Self {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(1);
        Self(NEXT_ID.fetch_add(1, Ordering::Relaxed))
    }
}

#[derive(Debug)]
pub(crate) enum EventKind {
    Readable,
    ReadClosed,
    ReadError,
    Writable,
    WriteClosed,
    WriteError,
}

impl EventKind {
    fn matches_interest(&self, interest: &Interest) -> bool {
        use EventKind::*;
        match self {
            Readable | ReadClosed => interest.is_readable(),
            Writable | WriteClosed => interest.is_writable(),
            // Always send error events
            ReadError | WriteError => true,
        }
    }
}

#[derive(Debug)]
pub struct Event {
    kind: EventKind,
    token: Token,
}

impl Event {
    pub(crate) fn new(kind: EventKind, token: Token) -> Self {
        Event { kind, token }
    }
}

pub type Events = Vec<Event>;

#[allow(clippy::trivially_copy_pass_by_ref)]
pub mod event {
    use super::EventKind;
    use crate::sys::Event;
    use crate::Token;
    use std::fmt;

    pub fn token(e: &Event) -> Token {
        e.token
    }

    pub fn is_readable(e: &Event) -> bool {
        match e.kind {
            EventKind::Readable | EventKind::ReadClosed | EventKind::ReadError => true,
            _ => false,
        }
    }

    pub fn is_writable(e: &Event) -> bool {
        match e.kind {
            EventKind::Writable | EventKind::WriteClosed | EventKind::WriteError => true,
            _ => false,
        }
    }

    pub fn is_error(e: &Event) -> bool {
        match e.kind {
            EventKind::ReadError | EventKind::WriteError => true,
            _ => false,
        }
    }

    pub fn is_read_closed(e: &Event) -> bool {
        match e.kind {
            EventKind::ReadClosed => true,
            _ => false,
        }
    }

    pub fn is_write_closed(e: &Event) -> bool {
        match e.kind {
            EventKind::WriteClosed => true,
            _ => false,
        }
    }

    pub fn is_priority(_: &Event) -> bool {
        false
    }

    pub fn is_aio(_: &Event) -> bool {
        false
    }

    pub fn is_lio(_: &Event) -> bool {
        false
    }

    pub fn debug_details(f: &mut fmt::Formatter<'_>, e: &Event) -> fmt::Result {
        fmt::Debug::fmt(e, f)
    }
}
