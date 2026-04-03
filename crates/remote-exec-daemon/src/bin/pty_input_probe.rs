use std::io::{self, BufRead, Write};
use std::thread;
use std::time::Duration;

fn escape_control_characters(input: &str) -> String {
    let mut escaped = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '\r' => escaped.push_str("\\r"),
            '\n' => escaped.push_str("\\n"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

fn main() -> io::Result<()> {
    let mode = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "read_line".to_string());
    let mut stdout = io::stdout().lock();
    match mode.as_str() {
        "read_line" => {
            stdout.write_all(b"READY\n")?;
            stdout.flush()?;

            let mut line = String::new();
            io::stdin().lock().read_line(&mut line)?;

            writeln!(stdout, "LINE:{}", escape_control_characters(&line))?;
            stdout.flush()?;
        }
        "delayed_tokens" => {
            thread::sleep(Duration::from_millis(400));
            stdout.write_all(b"one two three four five six")?;
            stdout.flush()?;
            thread::sleep(Duration::from_secs(30));
        }
        other => {
            writeln!(stdout, "unknown mode: {other}")?;
            stdout.flush()?;
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unsupported probe mode: {other}"),
            ));
        }
    }

    Ok(())
}
