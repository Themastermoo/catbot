//! Turns recordings into the list of distinct routes the interface used.

use std::{
    collections::{BTreeMap, BTreeSet},
    io::{BufRead, BufReader},
    path::PathBuf,
};

use serde_json::Value;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Default)]
struct Seen {
    count: usize,
    statuses: BTreeSet<u16>,
    example: String,
}

pub fn run(paths: &[PathBuf]) -> Result<(), BoxError> {
    let files = recording_files(paths)?;
    if files.is_empty() {
        return Err("no recordings found (run `catbot record` and use the app first)".into());
    }
    let mut routes: BTreeMap<(String, String), Seen> = BTreeMap::new();
    for file in &files {
        for line in BufReader::new(std::fs::File::open(file)?).lines() {
            let Ok(value) = serde_json::from_str::<Value>(&line?) else { continue };
            let method = match value["kind"].as_str() {
                Some("http") => value["method"].as_str().unwrap_or("?").to_string(),
                Some("sseopen") => "SSE".to_string(),
                Some("ws") => "WS".to_string(),
                _ => continue,
            };
            let Some(path) = value["path"].as_str() else { continue };
            let seen = routes.entry((pattern(path), method)).or_default();
            seen.count += 1;
            if let Some(status) = value["status"].as_u64() {
                seen.statuses.insert(status as u16);
            }
            if seen.example.is_empty() {
                seen.example = path.to_string();
            }
        }
    }

    println!("{} distinct routes from {} recording(s)\n", routes.len(), files.len());
    println!("{:<7} {:<60} {:>6}  STATUS", "METHOD", "ROUTE", "COUNT");
    for ((route, method), seen) in &routes {
        let statuses: Vec<String> = seen.statuses.iter().map(u16::to_string).collect();
        println!("{method:<7} {route:<60} {:>6}  {}", seen.count, statuses.join(","));
    }
    Ok(())
}

fn recording_files(paths: &[PathBuf]) -> Result<Vec<PathBuf>, BoxError> {
    let mut files = Vec::new();
    for path in paths {
        if path.is_dir() {
            for entry in std::fs::read_dir(path)? {
                let entry = entry?.path();
                if entry.extension().is_some_and(|ext| ext == "jsonl") {
                    files.push(entry);
                }
            }
        } else if path.exists() {
            files.push(path.clone());
        } else {
            return Err(format!("{} does not exist", path.display()).into());
        }
    }
    files.sort();
    Ok(files)
}

/// A path with its ids, numbers and file names replaced, so one route is listed once.
fn pattern(path: &str) -> String {
    let segments: Vec<&str> = path.split('/').map(|segment| if is_variable(segment) { ":id" } else { segment }).collect();
    segments.join("/")
}

fn is_variable(segment: &str) -> bool {
    if segment.is_empty() {
        return false;
    }
    let all_digits = segment.bytes().all(|b| b.is_ascii_digit());
    let uuid = segment.len() == 36 && segment.bytes().filter(|b| *b == b'-').count() == 4
        && segment.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-');
    let long_id = segment.len() >= 16
        && segment.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        && segment.bytes().any(|b| b.is_ascii_digit());
    all_digits || uuid || long_id || segment.contains('%') || segment.contains('.')
}

#[cfg(test)]
mod tests {
    use super::pattern;

    #[test]
    fn ids_collapse_to_one_route() {
        assert_eq!(pattern("/api/bots"), "/api/bots");
        assert_eq!(pattern("/api/attachments/123e4567-e89b-12d3-a456-426614174000"), "/api/attachments/:id");
        assert_eq!(pattern("/api/bots/42/messages"), "/api/bots/:id/messages");
        assert_eq!(pattern("/api/bots/bot_3f9a8c2d1e7b4a60/avatar"), "/api/bots/:id/avatar");
        assert_eq!(pattern("/api/files/photo.png"), "/api/files/:id");
        assert_eq!(pattern("/api/bot-defaults/avatar"), "/api/bot-defaults/avatar");
    }
}
