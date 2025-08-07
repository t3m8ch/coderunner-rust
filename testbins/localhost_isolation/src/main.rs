use std::net::TcpStream;

fn main() {
    match TcpStream::connect("127.0.0.1:22") {
        Ok(_) => std::process::exit(42),
        Err(_) => std::process::exit(0),
    }
}
