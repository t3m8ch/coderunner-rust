fn main() {
    match std::fs::read_to_string("/tmp/secret.txt") {
        Ok(_) => std::process::exit(42),
        Err(_) => std::process::exit(0),
    }
}
