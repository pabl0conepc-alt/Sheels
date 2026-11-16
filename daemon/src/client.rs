//! Thin, std-only client side (no async runtime → tiny memory footprint for
//! the long-running `listen` processes EWW keeps alive).

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

fn connect() -> Option<UnixStream> {
    UnixStream::connect(crate::util::socket_path()).ok()
}

/// Stream a topic forever. If the daemon is not up yet (or restarts), keep
/// retrying so EWW never sees the listener die.
pub fn listen(topic: &str) -> i32 {
    if topic.is_empty() {
        eprintln!("usage: sheels listen <topic>");
        return 2;
    }
    let stdout = std::io::stdout();
    let mut backoff = 100;
    loop {
        if let Some(mut s) = connect() {
            backoff = 100;
            if writeln!(s, "{}", serde_json::json!(["listen", topic])).is_ok() {
                let reader = BufReader::new(s);
                for line in reader.lines() {
                    let Ok(line) = line else { break };
                    let mut out = stdout.lock();
                    if writeln!(out, "{line}").and_then(|_| out.flush()).is_err() {
                        // EWW went away: exit quietly.
                        return 0;
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(backoff));
        backoff = (backoff * 2).min(2000);
    }
}

pub fn send(args: &[String]) -> i32 {
    let mut args = args.to_vec();
    // Text payloads (launcher query, wifi password…) can come through stdin so
    // EWW never has to interpolate user text into a shell command line.
    if args.len() == 2 && matches!((args[0].as_str(), args[1].as_str()), ("launcher", "query" | "accept") | ("wifi", "password"))
    {
        let mut buf = String::new();
        let _ = std::io::stdin().read_to_string(&mut buf);
        args.push(buf.trim_end_matches('\n').to_string());
    }
    let Some(mut s) = connect() else {
        eprintln!("sheels: daemon not running (start it with `sheels daemon`)");
        return 1;
    };
    let _ = s.set_read_timeout(Some(Duration::from_secs(10)));
    if writeln!(s, "{}", serde_json::Value::from(args.clone())).is_err() {
        return 1;
    }
    let mut reply = String::new();
    let _ = BufReader::new(s).read_line(&mut reply);
    let reply = reply.trim_end_matches(['\n', '\r']).replace('\u{1e}', "\n");
    let reply = reply.as_str();
    if let Some(err) = reply.strip_prefix("ERR ") {
        eprintln!("sheels: {err}");
        1
    } else {
        if !reply.is_empty() && reply != "ok" {
            println!("{reply}");
        }
        0
    }
}
