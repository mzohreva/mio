use std::io;
use PollOpt;

mod awakener;
mod selector;
mod tcp;
mod udp;

pub use self::awakener::Awakener;
pub use self::selector::{Events, Selector};
pub use self::tcp::{TcpListener, TcpStream};
pub use self::udp::UdpSocket;

fn other(s: &str) -> io::Error {
    io::Error::new(io::ErrorKind::Other, s)
}

fn would_block() -> io::Error {
    io::ErrorKind::WouldBlock.into()
}

fn check_opts(opts: PollOpt) -> io::Result<()> {
    if opts.is_level() || opts.is_oneshot() {
        return Err(other(
            "Oneshot and level-triggered events are not supported in SGX",
        ));
    }
    Ok(())
}
