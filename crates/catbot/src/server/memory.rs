//! The memory vault, read-only, for the interface at home and paired phones away from it.
//!
//! - `GET /api/memory`: whether the vault is on, and its notes (not the Codex's).
//! - `GET /api/memory/note?path=Bots/Scout.md`: one note, without its frontmatter.
//! - `GET /api/memory/note?link=Scout&from=Journal/2026-11-15 Scout.md`: the note a
//!   `[[wikilink]]` leads to, found the way Obsidian finds it (by path, else by name), looking
//!   in the Codex first when the link was in a Codex note.
//!
//! Only Markdown notes inside the vault or the Codex it links to can be read; nothing is written.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use axum::{extract::Request, http::StatusCode};
use serde_json::{Value, json};

use super::{Answer, App, query_param};
use crate::memory::CODEX_LINK;

/// Notes listed at most (the vault is small; this only guards against a runaway folder).
const MOST: usize = 2000;

pub fn route(app: &Arc<App>, rest: &[&str], req: &Request) -> Answer {
    let Some(vault) = app.memory.clone() else {
        return match rest {
            [] => Ok((StatusCode::OK, json!({ "on": false, "notes": [] }))),
            _ => Err((StatusCode::NOT_FOUND, "the memory vault is off".into())),
        };
    };
    match rest {
        [] => Ok((StatusCode::OK, overview(&vault))),
        ["note"] => {
            let path = query_param(req, "path").map(decode);
            let link = query_param(req, "link").map(decode);
            let from = query_param(req, "from").map(decode).unwrap_or_default();
            let found = match (path, link) {
                (Some(path), _) => readable(&vault, &path).map(|p| (path, p)),
                (None, Some(link)) => resolve(&vault, &link, &from),
                (None, None) => return Err((StatusCode::BAD_REQUEST, "give a path or a link".into())),
            };
            let (path, file) = found.ok_or((StatusCode::NOT_FOUND, "no such note".into()))?;
            let text = std::fs::read_to_string(&file).map_err(|err| (StatusCode::NOT_FOUND, err.to_string()))?;
            Ok((
                StatusCode::OK,
                json!({
                    "path": path,
                    "title": title(&path),
                    "text": without_frontmatter(&text),
                    "modified": modified(&file),
                    "inCodex": in_codex(&path),
                }),
            ))
        }
        _ => Err((StatusCode::NOT_FOUND, "no such memory route".into())),
    }
}

fn overview(vault: &Path) -> Value {
    let mut notes = Vec::new();
    walk(vault, vault, false, &mut notes);
    notes.sort();
    let notes: Vec<Value> = notes
        .into_iter()
        .map(|rel| {
            let folder = rel.split_once('/').map(|(f, _)| f).unwrap_or("").to_string();
            json!({ "path": rel, "title": title(&rel), "folder": folder, "modified": modified(&vault.join(&rel)) })
        })
        .collect();
    json!({
        "on": true,
        "path": vault.display().to_string(),
        "codex": crate::memory::has_codex(vault),
        "notes": notes,
    })
}

/// Collects notes under `dir` as paths relative to `root`, skipping hidden folders and, unless
/// `codex`, the Codex link.
fn walk(root: &Path, dir: &Path, codex: bool, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        if out.len() >= MOST {
            return;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            if !codex && dir == root && name == CODEX_LINK {
                continue;
            }
            walk(root, &path, codex, out);
        } else if name.ends_with(".md")
            && let Ok(rel) = path.strip_prefix(root)
        {
            out.push(rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/"));
        }
    }
}

/// The file for a note path, if it's a Markdown note inside the vault or its Codex.
fn readable(vault: &Path, rel: &str) -> Option<PathBuf> {
    if !rel.ends_with(".md") || rel.contains('\\') {
        return None;
    }
    let rel_path = Path::new(rel);
    if !rel_path.components().all(|c| matches!(c, Component::Normal(_))) {
        return None;
    }
    let file = vault.join(rel_path).canonicalize().ok()?;
    let home = vault.canonicalize().ok()?;
    let codex = vault.join(CODEX_LINK).canonicalize().ok();
    let inside = file.starts_with(&home) || codex.is_some_and(|c| file.starts_with(c));
    (inside && file.is_file()).then_some(file)
}

/// Where a `[[link]]` goes: by path, else by note name.
fn resolve(vault: &Path, link: &str, from: &str) -> Option<(String, PathBuf)> {
    let target = link.split(['#', '|']).next().unwrap_or("").trim();
    let target = target.strip_suffix(".md").unwrap_or(target);
    if target.is_empty() {
        return None;
    }
    let codex_first = in_codex(from);
    // A path: as written, then inside the Codex if the link was there.
    if target.contains('/') {
        let mut tries = vec![format!("{target}.md")];
        let in_codex_path = format!("{CODEX_LINK}/{target}.md");
        if codex_first {
            tries.insert(0, in_codex_path);
        } else {
            tries.push(in_codex_path);
        }
        for path in tries {
            if let Some(file) = readable(vault, &path) {
                return Some((path, file));
            }
        }
    }
    // A name: the vault's own notes and the Codex's, nearest first.
    let name = target.rsplit('/').next().unwrap_or(target).to_lowercase();
    let mut own = Vec::new();
    walk(vault, vault, false, &mut own);
    let mut codex = Vec::new();
    if crate::memory::has_codex(vault) {
        walk(vault, &vault.join(CODEX_LINK), true, &mut codex);
    }
    let order = if codex_first { [codex, own] } else { [own, codex] };
    order
        .into_iter()
        .flatten()
        .find(|path| title(path).to_lowercase() == name)
        .and_then(|path| readable(vault, &path).map(|file| (path, file)))
}

fn in_codex(path: &str) -> bool {
    path.starts_with(&format!("{CODEX_LINK}/"))
}

fn title(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.strip_suffix(".md").unwrap_or(name).to_string()
}

fn modified(file: &Path) -> Option<String> {
    let time = std::fs::metadata(file).ok()?.modified().ok()?;
    Some(chrono::DateTime::<chrono::Local>::from(time).to_rfc3339_opts(chrono::SecondsFormat::Secs, false))
}

/// The note without the `---` block at its top.
fn without_frontmatter(text: &str) -> &str {
    let Some(rest) = text.strip_prefix("---\n").or_else(|| text.strip_prefix("---\r\n")) else { return text };
    for end in ["\n---\n", "\n---\r\n"] {
        if let Some(at) = rest.find(end) {
            return rest[at + end.len()..].trim_start_matches(['\n', '\r']);
        }
    }
    if rest.ends_with("\n---") {
        return "";
    }
    text
}

/// Undoes the browser's percent-encoding of a query value.
fn decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (b'+', _) => {
                out.push(b' ');
                i += 1;
            }
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let codex = dir.path().join("codex");
        std::fs::create_dir_all(codex.join("20 Projects")).unwrap();
        std::fs::write(codex.join("20 Projects/Catbot.md"), "---\ntype: project\n---\n# Catbot\n").unwrap();
        std::fs::write(codex.join("Scout.md"), "the Codex's Scout").unwrap();
        std::fs::write(dir.path().join("secret.md"), "outside").unwrap();
        let vault = dir.path().join("memory");
        crate::memory::prepare(&vault, Some(&codex)).unwrap();
        std::fs::write(vault.join("Bots/Scout.md"), "---\nbot: Scout\n---\n\n# Scout\n").unwrap();
        (dir, vault)
    }

    /// Wine can't make the Codex link (Windows makes a junction); the Codex checks need it.
    fn linked(vault: &Path) -> bool {
        let linked = crate::memory::has_codex(vault);
        if !linked {
            #[cfg(unix)]
            panic!("the Codex link is always made on Linux");
            #[cfg(not(unix))]
            eprintln!("skipped: no Codex link here");
        }
        linked
    }

    #[test]
    fn the_list_leaves_out_the_codex() {
        let (_dir, vault) = vault();
        if !linked(&vault) {
            return;
        }
        let list = overview(&vault);
        let paths: Vec<&str> = list["notes"].as_array().unwrap().iter().map(|n| n["path"].as_str().unwrap()).collect();
        assert_eq!(paths, ["AGENTS.md", "Bots/Scout.md", "Home.md"]);
        assert_eq!(list["codex"], true);
    }

    #[test]
    fn only_notes_inside_can_be_read() {
        let (_dir, vault) = vault();
        if !linked(&vault) {
            return;
        }
        assert!(readable(&vault, "Bots/Scout.md").is_some());
        assert!(readable(&vault, "Codex/20 Projects/Catbot.md").is_some(), "the Codex, through the link");
        for path in ["../secret.md", "/etc/passwd.md", "Bots/../../secret.md", "Bots", "Home", "Bots\\Scout.md"] {
            assert!(readable(&vault, path).is_none(), "{path}");
        }
    }

    #[test]
    fn links_resolve_like_obsidian() {
        let (_dir, vault) = vault();
        if !linked(&vault) {
            return;
        }
        assert_eq!(resolve(&vault, "Scout", "Home.md").unwrap().0, "Bots/Scout.md");
        assert_eq!(resolve(&vault, "Scout", "Codex/20 Projects/Catbot.md").unwrap().0, "Codex/Scout.md", "nearest first");
        assert_eq!(resolve(&vault, "Codex/20 Projects/Catbot|Catbot", "Bots/Scout.md").unwrap().0, "Codex/20 Projects/Catbot.md");
        assert_eq!(resolve(&vault, "20 Projects/Catbot#Plan", "Codex/Scout.md").unwrap().0, "Codex/20 Projects/Catbot.md");
        assert_eq!(resolve(&vault, "catbot", "Home.md").unwrap().0, "Codex/20 Projects/Catbot.md");
        assert!(resolve(&vault, "Nowhere", "Home.md").is_none());
    }

    #[test]
    fn frontmatter_and_query_values() {
        assert_eq!(without_frontmatter("---\na: 1\n---\n\n# T\n"), "# T\n");
        assert_eq!(without_frontmatter("# No frontmatter"), "# No frontmatter");
        assert_eq!(decode("Bots%2FScout%20Two.md"), "Bots/Scout Two.md");
        assert_eq!(decode("a+b%"), "a b%");
        assert_eq!(decode("%E2%80%94"), "—");
    }
}
