fn main() {
    let size = 50 * 1024 * 1024;
    let mut v = Vec::with_capacity(size);
    for i in 0..size {
        v.push(i as u8);
    }
    std::thread::sleep(std::time::Duration::from_secs(5));
}
