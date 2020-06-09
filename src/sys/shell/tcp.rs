use std::io;
use std::net::SocketAddr;

pub use std::net::{TcpListener, TcpStream};

pub fn connect(_: SocketAddr) -> io::Result<TcpStream> {
    os_required!();
}

pub fn bind(_: SocketAddr) -> io::Result<TcpListener> {
    os_required!();
}

pub fn accept(_: &TcpListener) -> io::Result<(TcpStream, SocketAddr)> {
    os_required!();
}
