use event::Evented;
use std::net::{self, Ipv4Addr, Ipv6Addr, SocketAddr};
use {io, Poll, PollOpt, Ready, Token};

#[derive(Debug)]
pub struct UdpSocket;

impl UdpSocket {
    pub fn new(_socket: net::UdpSocket) -> io::Result<UdpSocket> {
        unimplemented!()
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        unimplemented!()
    }

    pub fn try_clone(&self) -> io::Result<UdpSocket> {
        unimplemented!()
    }

    pub fn send_to(&self, _buf: &[u8], _target: &SocketAddr) -> io::Result<usize> {
        unimplemented!()
    }

    pub fn recv_from(&self, _buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        unimplemented!()
    }

    pub fn send(&self, _buf: &[u8]) -> io::Result<usize> {
        unimplemented!()
    }

    pub fn recv(&self, _buf: &mut [u8]) -> io::Result<usize> {
        unimplemented!()
    }

    pub fn connect(&self, _addr: SocketAddr) -> io::Result<()> {
        unimplemented!()
    }

    pub fn broadcast(&self) -> io::Result<bool> {
        unimplemented!()
    }

    pub fn set_broadcast(&self, _on: bool) -> io::Result<()> {
        unimplemented!()
    }

    pub fn multicast_loop_v4(&self) -> io::Result<bool> {
        unimplemented!()
    }

    pub fn set_multicast_loop_v4(&self, _on: bool) -> io::Result<()> {
        unimplemented!()
    }

    pub fn multicast_ttl_v4(&self) -> io::Result<u32> {
        unimplemented!()
    }

    pub fn set_multicast_ttl_v4(&self, _ttl: u32) -> io::Result<()> {
        unimplemented!()
    }

    pub fn multicast_loop_v6(&self) -> io::Result<bool> {
        unimplemented!()
    }

    pub fn set_multicast_loop_v6(&self, _on: bool) -> io::Result<()> {
        unimplemented!()
    }

    pub fn ttl(&self) -> io::Result<u32> {
        unimplemented!()
    }

    pub fn set_ttl(&self, _ttl: u32) -> io::Result<()> {
        unimplemented!()
    }

    pub fn join_multicast_v4(&self, _: &Ipv4Addr, _: &Ipv4Addr) -> io::Result<()> {
        unimplemented!()
    }

    pub fn join_multicast_v6(&self, _: &Ipv6Addr, _: u32) -> io::Result<()> {
        unimplemented!()
    }

    pub fn leave_multicast_v4(&self, _: &Ipv4Addr, _: &Ipv4Addr) -> io::Result<()> {
        unimplemented!()
    }

    pub fn leave_multicast_v6(&self, _: &Ipv6Addr, _: u32) -> io::Result<()> {
        unimplemented!()
    }

    pub fn set_only_v6(&self, _only_v6: bool) -> io::Result<()> {
        unimplemented!()
    }

    pub fn only_v6(&self) -> io::Result<bool> {
        unimplemented!()
    }

    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        unimplemented!()
    }
}

impl Evented for UdpSocket {
    fn register(
        &self,
        _poll: &Poll,
        _token: Token,
        _interest: Ready,
        _opts: PollOpt,
    ) -> io::Result<()> {
        unimplemented!()
    }

    fn reregister(
        &self,
        _poll: &Poll,
        _token: Token,
        _interest: Ready,
        _opts: PollOpt,
    ) -> io::Result<()> {
        unimplemented!()
    }

    fn deregister(&self, _poll: &Poll) -> io::Result<()> {
        unimplemented!()
    }
}
