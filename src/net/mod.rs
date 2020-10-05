//! Networking primitives
//!
//! The types provided in this module are non-blocking by default and are
//! designed to be portable across all supported Mio platforms. As long as the
//! [portability guidelines] are followed, the behavior should be identical no
//! matter the target platform.
//!
//! [portability guidelines]: ../struct.Poll.html#portability

cfg_tcp! {
    mod tcp;
    pub use self::tcp::{TcpListener, TcpStream};
}

#[cfg(not(target_env = "sgx"))]
cfg_udp! {
    mod udp;
    pub use self::udp::UdpSocket;
}

#[cfg(unix)]
cfg_uds! {
    mod uds;
    pub use self::uds::{SocketAddr, UnixDatagram, UnixListener, UnixStream};
}
