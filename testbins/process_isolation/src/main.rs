use std::process::Command;

fn main() {
    match Command::new("ps").arg("aux").output() {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if stdout.lines().count() > 5 {
                eprintln!("Too many processes");
                if std::process::id() != 1 {
                    eprintln!("PID != 1");
                }
                std::process::exit(42);
            }

            if std::process::id() == 1 {
                std::process::exit(0);
            } else {
                eprintln!("PID != 1");
                std::process::exit(42);
            }
        }
        Err(_) => std::process::exit(0),
    }
}
