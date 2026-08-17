//! Test fixture spawned by hook e2e / runner tests.

use std::io::{Read, Write};
use std::time::Duration;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut stdin = String::new();
    let _ = std::io::stdin().read_to_string(&mut stdin);
    match args.first().map(String::as_str) {
        Some("deny") => {
            let reason = args.get(1).cloned().unwrap_or_else(|| "denied".into());
            let reason = serde_json::to_string(&reason).unwrap_or_else(|_| "\"denied\"".into());
            println!("{{\"decision\":\"deny\",\"reason\":{reason}}}");
        }
        Some("allow") => println!("{{\"decision\":\"allow\"}}"),
        Some("empty") => {}
        Some("hang") => std::thread::sleep(Duration::from_secs(30)),
        Some("exit1") => {
            let _ = writeln!(std::io::stderr(), "hook crashed");
            std::process::exit(1);
        }
        Some("touch") => {
            if let Some(path) = args.get(1) {
                let _ = std::fs::write(path, "ran");
            }
            println!("{{\"decision\":\"allow\"}}");
        }
        _ => println!("{{\"decision\":\"allow\"}}"),
    }
}
