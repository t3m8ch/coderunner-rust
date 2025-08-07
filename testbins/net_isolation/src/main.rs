use std::net::TcpStream;

fn main() {
    match TcpStream::connect("8.8.8.8:53") {
        Ok(_) => std::process::exit(42),
        Err(_) => std::process::exit(0),
    }
}
