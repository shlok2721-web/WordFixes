//! Serves a vault directory over WebDAV, for testing client mode against a
//! second machine without needing a second machine.
//!
//!     cargo run --example serve -- /path/to/vault 18140 [user] [pass]

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let root = args.next().expect("usage: serve <vault-dir> <port> [user] [pass]");
    let port: u16 = args.next().expect("port").parse()?;
    let username = args.next().unwrap_or_else(|| "u".into());
    let password = args.next().unwrap_or_else(|| "p".into());

    let handle = notes::server::start(notes::server::ServerConfig {
        bind: "127.0.0.1".into(),
        port,
        username,
        password,
        root: root.into(),
    })?;
    println!("serving on http://127.0.0.1:{port} — Ctrl-C to stop");
    while handle.is_running() {
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    Ok(())
}
