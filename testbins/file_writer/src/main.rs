fn main() {
    std::fs::write("/hello", "world!").expect("Failed to write file");
}
