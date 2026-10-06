//! Small local HTTP service for the native model.
//!
//! The service keeps the immutable deck data in an `Arc` and gives each request
//! its own native OS thread. This makes concurrent searches independent while
//! avoiding a reload of the usually large deck-data file.

use ournotes_search::parallel;
use ournotes_sim::data::DeckData;
use serde_json::{Value, json};
use std::{
    env,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::Arc,
    thread,
};

fn response(status: &str, body: String) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn handle(mut stream: TcpStream, data: Arc<DeckData>) {
    let mut raw = Vec::new();
    let split = loop {
        let mut chunk = [0u8; 4096];
        let n = match stream.read(&mut chunk) {
            Ok(n) => n,
            Err(_) => return,
        };
        if n == 0 {
            return;
        }
        raw.extend_from_slice(&chunk[..n]);
        if let Some(split) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
            break split;
        }
        if raw.len() > 64 * 1024 {
            return;
        }
    };
    let headers = String::from_utf8_lossy(&raw[..split]);
    let content_length = headers
        .lines()
        .find_map(|line| line.strip_prefix("Content-Length:").and_then(|v| v.trim().parse::<usize>().ok()))
        .unwrap_or(0);
    while raw.len() < split + 4 + content_length {
        let mut chunk = [0u8; 4096];
        let n = match stream.read(&mut chunk) {
            Ok(n) => n,
            Err(_) => return,
        };
        if n == 0 {
            return;
        }
        raw.extend_from_slice(&chunk[..n]);
    }
    let head = String::from_utf8_lossy(&raw[..split]);
    let body = &raw[split + 4..split + 4 + content_length];
    let mut lines = head.lines();
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let path = parts.next().unwrap_or_default();
    let out = match (method, path) {
        ("GET", "/health") => (
            "200 OK",
            json!({"ok":true,"threads":{"min":1,"max":parallel::max_workers(),"default":parallel::default_workers()}})
                .to_string(),
        ),
        ("POST", "/recommend") => match serde_json::from_slice::<Value>(body) {
            Ok(input) => {
                let roster = input.get("roster").map(Value::to_string);
                let request = input.get("request").map(Value::to_string);
                let threads = match input.get("threads") {
                    None => Ok(parallel::default_workers()),
                    Some(value) => value
                        .as_u64()
                        .and_then(|v| usize::try_from(v).ok())
                        .ok_or_else(|| "threads must be a positive integer".to_string())
                        .and_then(|n| parallel::validate_workers(n).map(|()| n).map_err(|e| e.to_string())),
                };
                match (roster, request, threads) {
                    (_, _, Err(error)) => ("422 Unprocessable Entity", json!({"error":error}).to_string()),
                    (Some(roster), Some(request), Ok(threads)) => {
                        match parallel::recommend_json_with_threads(&data, &roster, &request, threads) {
                            Ok(result) => ("200 OK", result),
                            Err(error) => ("422 Unprocessable Entity", json!({"error":error.to_string()}).to_string()),
                        }
                    }
                    _ => ("400 Bad Request", json!({"error":"body requires roster and request"}).to_string()),
                }
            }
            Err(error) => ("400 Bad Request", json!({"error":error.to_string()}).to_string()),
        },
        _ => ("404 Not Found", json!({"error":"not found"}).to_string()),
    };
    let _ = stream.write_all(response(out.0, out.1).as_bytes());
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let data_path = env::var("OURNOTES_DECK_DATA").map_err(|_| "OURNOTES_DECK_DATA is required")?;
    let bind = env::var("OURNOTES_BIND").unwrap_or_else(|_| "127.0.0.1:18766".into());
    let data = Arc::new(DeckData::from_path(data_path)?);
    let listener = TcpListener::bind(&bind)?;
    eprintln!("ournotes-service listening on {bind}");
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let data = Arc::clone(&data);
                thread::spawn(move || handle(stream, data));
            }
            Err(error) => eprintln!("accept: {error}"),
        }
    }
    Ok(())
}
