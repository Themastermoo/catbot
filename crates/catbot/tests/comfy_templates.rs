//! Catbot's workflow converter against ComfyUI's own example workflows, checked by a real ComfyUI.
//!
//! Skipped unless both are given:
//!   COMFY_URL=http://127.0.0.1:8188 COMFY_TEMPLATES=<…/comfyui_workflow_templates_json/templates>
//!   cargo test -p catbot --test comfy_templates -- --nocapture
//! Each example is converted and sent to ComfyUI's /prompt. A model or picture that isn't on this
//! computer is fine (the examples name files nobody has); anything that says the conversion put a
//! value in the wrong place, or left a needed one out, fails the test. ComfyUI may run an example
//! that needs nothing missing, so point it at a ComfyUI you don't mind being busy.

use serde_json::{Value, json};

/// Kinds of complaint that mean the conversion was wrong, not that a file is missing.
const WRONG: [&str; 6] = ["required_input_missing", "invalid_input_type", "return_type_mismatch", "bad_linked_input", "value_smaller_than_min", "value_bigger_than_max"];

fn looks_like_a_file(v: &Value) -> bool {
    let s = match v {
        Value::String(s) => s.as_str(),
        _ => return false,
    };
    s.contains('.') && s.rsplit('.').next().is_some_and(|ext| ext.len() <= 12 && ext.chars().all(|c| c.is_ascii_alphanumeric()))
}

#[tokio::test]
async fn every_example_converts() {
    let (Ok(url), Ok(folder)) = (std::env::var("COMFY_URL"), std::env::var("COMFY_TEMPLATES")) else {
        eprintln!("skipped: set COMFY_URL and COMFY_TEMPLATES");
        return;
    };
    let client = reqwest::Client::new();
    let info: Value = client.get(format!("{url}/object_info")).send().await.unwrap().json().await.unwrap();
    let mut files: Vec<_> = std::fs::read_dir(&folder).unwrap().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect();
    files.sort();
    let (mut tried, mut fine, mut not_installed, mut unfinished) = (0, 0, 0, 0);
    let mut wrong = Vec::new();
    for path in files {
        let Ok(saved) = serde_json::from_str::<Value>(&std::fs::read_to_string(&path).unwrap()) else { continue };
        if !catbot::server::comfy_graph::is_editor_form(&saved) {
            continue;
        }
        tried += 1;
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let run = match catbot::server::comfy_graph::to_run_form(&saved, &info) {
            Ok(run) => run,
            Err(why) if why.contains("isn't installed") => {
                not_installed += 1;
                continue;
            }
            // The example itself is unfinished; saying so is the right answer.
            Err(why) if why.contains("isn't wired to anything") => {
                eprintln!("  (unfinished example) {name}: {why}");
                unfinished += 1;
                continue;
            }
            Err(why) => {
                wrong.push(format!("{name}: {why}"));
                continue;
            }
        };
        let answer: Value = client.post(format!("{url}/prompt")).json(&json!({ "prompt": run, "client_id": "catbot-test" })).send().await.unwrap().json().await.unwrap();
        let mut bad = Vec::new();
        for (id, node) in answer["node_errors"].as_object().into_iter().flatten() {
            for e in node["errors"].as_array().into_iter().flatten() {
                let kind = e["type"].as_str().unwrap_or_default();
                let received = &e["extra_info"]["received_value"];
                let missing_file = kind == "value_not_in_list" && (looks_like_a_file(received) || received.as_str().is_some_and(|s| s.contains('/') || s.is_empty()));
                if WRONG.contains(&kind) || (kind == "value_not_in_list" && !missing_file) {
                    bad.push(format!("{id} {}: {kind} {} (got {received})", node["class_type"].as_str().unwrap_or_default(), e["extra_info"]["input_name"].as_str().unwrap_or_default()));
                }
            }
        }
        if answer["error"]["type"] == "prompt_no_outputs" {
            bad.push("no outputs".into());
        }
        if bad.is_empty() {
            fine += 1;
        } else {
            wrong.push(format!("{name}: {}", bad.join("; ")));
        }
    }
    // Nothing left running here.
    let _ = client.post(format!("{url}/queue")).json(&json!({ "clear": true })).send().await;
    let _ = client.post(format!("{url}/interrupt")).send().await;
    eprintln!("{tried} examples: {fine} converted right, {not_installed} need nodes this ComfyUI lacks, {unfinished} unfinished, {} wrong", wrong.len());
    for w in wrong.iter().take(60) {
        eprintln!("  {w}");
    }
    assert!(wrong.is_empty(), "{} examples converted wrong", wrong.len());
}
