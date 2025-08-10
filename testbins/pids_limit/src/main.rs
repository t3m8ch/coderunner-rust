use std::process::{Child, Command};

fn main() {
    let mut children: Vec<Child> = Vec::new();

    for _ in 0..30 {
        let result = Command::new("sleep").arg("1").spawn();
        match result {
            Ok(child) => children.push(child),
            Err(err) => eprintln!("Failed to spawn child: {}", err),
        }
    }

    for mut child in children {
        child.wait().unwrap();
    }
}
