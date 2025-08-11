use std::io::{self, Write};

fn main() -> io::Result<()> {
    let size = 2 * 1024 * 1024;
    let buffer = vec![b'a'; size];

    let mut stderr = io::stderr();
    stderr.write_all(&buffer)?;
    stderr.flush()?;

    Ok(())
}
