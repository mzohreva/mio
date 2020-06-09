
#[cfg(not(target_env = "sgx"))]
include!("./udp/udp_server.rs");

#[cfg(target_env = "sgx")]
fn main() {
    println!("SGX does not support UDP yet");
}
