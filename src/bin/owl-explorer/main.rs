mod api;

use crate::api::models::Registry;
use crate::api::router::handle_connection;
use std::fs;
use std::net::TcpListener;
use std::path::PathBuf;

const DEFAULT_BIND: &str = "127.0.0.1:4000";

fn registry_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".owl/explorer.json")
}

fn main() {
    let registry_file = registry_path();
    if let Some(parent) = registry_file.parent() {
        let _ = fs::create_dir_all(parent);
    }

    let mut registry = Registry::load(&registry_file);

    let bind = std::env::args()
        .nth(1)
        .unwrap_or_else(|| DEFAULT_BIND.to_string());
    let listener = TcpListener::bind(&bind).unwrap_or_else(|e| {
        panic!("failed to bind {}: {}", bind, e);
    });

    println!("Owl Explorer running at http://{}", bind);
    println!("Registry: {}", registry_file.display());

    for incoming in listener.incoming() {
        match incoming {
            Ok(stream) => {
                handle_connection(stream, &mut registry, &registry_file);
            }
            Err(e) => {
                eprintln!("connection error: {}", e);
            }
        }
    }
}
