//! ComfyUI workflows, from the form ComfyUI saves to the form it runs (stage 26).
//!
//! ComfyUI saves a workflow as its editor draws it (nodes, wires, widget values in screen
//! order, subgraphs, notes, reroutes, bypassed nodes); its `/prompt` takes another form, a node id
//! to `{class_type, inputs}`. The editor turns one into the other before it runs anything; this does
//! the same here, so bots can run the person's saved workflows. Which widget value belongs to
//! which input comes from ComfyUI's own description of each node (`/object_info`), as the editor
//! reads it, including the dynamic inputs of its newer nodes (`mode.yaw`).
//!
//! Tested against ComfyUI 0.46's own example workflows (`tests/comfy_templates.rs`, when a ComfyUI
//! runs nearby).

use std::collections::HashMap;

use serde_json::{Map, Value, json};

/// Kinds of input the editor draws as a widget (a value of its own) rather than a socket.
const WIDGET_TYPES: [&str; 6] = ["INT", "FLOAT", "STRING", "BOOLEAN", "COMBO", "COLOR"];
const DYNAMIC_COMBO: &str = "COMFY_DYNAMICCOMBO_V3";
/// Kinds the editor draws with a widget of its own making (a 3D view, a compare slider, a box
/// editor), whose value it saves like any other.
const CUSTOM_WIDGETS: [&str; 6] = ["LOAD_3D", "LOAD_3D_ANIMATION", "IMAGECOMPARE", "COMPOSITOR", "BOUNDING_BOXES", "WEBCAM"];
/// Values of the upload buttons the editor saves after a file picker.
const BUTTONS: [&str; 8] = ["image", "video", "audio", "file", "upload", "upload3dmodel", "uploadExtraResources", "clear"];
/// What the editor keeps after a seed: what to do with it after each run.
const CONTROLS: [&str; 4] = ["fixed", "increment", "decrement", "randomize"];
/// The editor's own nodes, which ComfyUI never runs.
const NOTES: [&str; 4] = ["Note", "MarkdownNote", "Label (rgthree)", "Note Plus (mtb)"];
/// The ids a subgraph's inside uses for its own inputs and outputs.
const SUB_IN: i64 = -10;
const SUB_OUT: i64 = -20;
/// Deepest subgraphs inside subgraphs.
const DEEPEST: usize = 16;

/// A saved workflow (`nodes` and `links`), not one already in the running form.
pub fn is_editor_form(v: &Value) -> bool {
    v["nodes"].is_array()
}

/// Already in the running form: every entry is a node with a `class_type`.
pub fn is_run_form(v: &Value) -> bool {
    v.as_object().is_some_and(|m| !m.is_empty() && m.values().all(|n| n["class_type"].is_string() && n["inputs"].is_object()))
}

/// Where a link starts.
#[derive(Clone, Copy)]
struct Link {
    origin: i64,
    origin_slot: usize,
}

/// One graph: the top, or a subgraph's inside.
struct Graph<'a> {
    nodes: Vec<&'a Value>,
    links: HashMap<i64, Link>,
    /// A subgraph's own inputs and outputs (none at the top).
    sub: Option<&'a Value>,
}

impl<'a> Graph<'a> {
    fn of(v: &'a Value, sub: Option<&'a Value>) -> Graph<'a> {
        let mut links = HashMap::new();
        for l in v["links"].as_array().into_iter().flatten() {
            // At the top a link is [id, origin, origin slot, target, target slot, type]; in a
            // subgraph, an object with those names.
            let (id, origin, slot) = if let Some(a) = l.as_array() {
                (a.first().and_then(Value::as_i64), a.get(1).and_then(Value::as_i64), a.get(2).and_then(Value::as_u64))
            } else {
                (l["id"].as_i64(), l["origin_id"].as_i64(), l["origin_slot"].as_u64())
            };
            if let (Some(id), Some(origin), Some(slot)) = (id, origin, slot) {
                links.insert(id, Link { origin, origin_slot: slot as usize });
            }
        }
        Graph { nodes: v["nodes"].as_array().map(|n| n.iter().collect()).unwrap_or_default(), links, sub }
    }

    fn node(&self, id: i64) -> Option<&'a Value> {
        self.nodes.iter().copied().find(|n| n["id"].as_i64() == Some(id))
    }
}

/// Where we are: a graph, and how to reach the graph around it.
struct Scope<'a, 'b> {
    graph: &'b Graph<'a>,
    /// The ids its nodes get in the running form: "" at the top, "12:" inside node 12.
    prefix: String,
    /// The node this subgraph stands in for, and the scope it's in.
    outer: Option<(&'a Value, &'b Scope<'a, 'b>)>,
    /// The instance's own widget values, by its subgraph input's name.
    values: HashMap<String, Value>,
}

/// Where an input gets its value: another node's output, a value, or nowhere.
enum Source {
    Output(String, usize),
    Value(Value),
    Nothing,
}

struct Converter<'a> {
    info: &'a Value,
    subgraphs: HashMap<String, &'a Value>,
    out: Map<String, Value>,
    problems: Vec<String>,
}

fn id_of(n: &Value) -> i64 {
    n["id"].as_i64().unwrap_or_default()
}

fn kind(n: &Value) -> &str {
    n["type"].as_str().unwrap_or_default()
}

/// 0 runs, 2 is muted (never runs), 4 is bypassed (its input passes straight through).
fn mode(n: &Value) -> i64 {
    n["mode"].as_i64().unwrap_or(0)
}

fn spec_type(spec: &Value) -> Option<&str> {
    match &spec[0] {
        Value::Array(_) => Some("COMBO"),
        Value::String(s) => Some(s.as_str()),
        _ => None,
    }
}

/// An input's description makes a widget in the editor.
fn is_widget(spec: &Value) -> bool {
    let options = &spec[1];
    if options["forceInput"] == true {
        return false;
    }
    if options["socketless"] == true || options["socketless"] == false && options.get("default").is_some() || options["widgetType"].is_string() {
        return true;
    }
    spec_type(spec).is_some_and(|t| WIDGET_TYPES.contains(&t) || t == DYNAMIC_COMBO || CUSTOM_WIDGETS.contains(&t))
}

/// A node's inputs in the order the editor draws them: required, then optional.
fn ordered(inputs: &Value, order: Option<&Value>) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    for section in ["required", "optional"] {
        let specs = &inputs[section];
        let names: Vec<String> = match order.and_then(|o| o[section].as_array()) {
            Some(names) => names.iter().filter_map(|n| n.as_str().map(str::to_string)).collect(),
            None => specs.as_object().map(|m| m.keys().cloned().collect()).unwrap_or_default(),
        };
        for name in names {
            if let Some(spec) = specs.get(&name) {
                out.push((name, spec.clone()));
            }
        }
    }
    out
}

/// Gives each widget its saved value, in the editor's order, into `out` (dynamic inputs as
/// "parent.child").
fn fill_widgets(inputs: &[(String, Value)], prefix: &str, values: &mut std::iter::Peekable<std::slice::Iter<'_, Value>>, named: Option<&Map<String, Value>>, out: &mut Map<String, Value>) {
    for (name, spec) in inputs {
        if !is_widget(spec) {
            continue;
        }
        let full = if prefix.is_empty() { name.clone() } else { format!("{prefix}.{name}") };
        let value = match named {
            Some(m) => m.get(&full).or_else(|| m.get(name)).cloned(),
            None => values.next().cloned(),
        };
        let value = value.or_else(|| spec[1].get("default").cloned());
        // A display the editor saves nothing for still has to be there.
        let value = value.or_else(|| (spec[1]["socketless"] == true).then(|| json!({})));
        let Some(value) = value else { continue };
        // A file picker's buttons are kept after it.
        if named.is_none() && spec[1].as_object().is_some_and(|o| o.keys().any(|k| k.ends_with("_upload"))) {
            while values.peek().and_then(|v| v.as_str()).is_some_and(|s| BUTTONS.contains(&s)) {
                values.next();
            }
        }
        // The seed's "after each run" choice is kept right after it.
        let int_like = matches!(spec_type(spec), Some("INT") | Some("COMBO"));
        if named.is_none()
            && int_like
            && (spec[1]["control_after_generate"] == true || name == "seed" || name == "noise_seed")
            && values.peek().and_then(|v| v.as_str()).is_some_and(|s| CONTROLS.contains(&s))
        {
            values.next();
        }
        if spec_type(spec) == Some(DYNAMIC_COMBO) {
            let key = value.as_str().unwrap_or_default().to_string();
            out.insert(full.clone(), value);
            let chosen = spec[1]["options"].as_array().and_then(|o| o.iter().find(|x| x["key"] == key.as_str()));
            if let Some(chosen) = chosen {
                let inner = ordered(&chosen["inputs"], None);
                fill_widgets(&inner, &full, values, named, out);
            }
            continue;
        }
        // A list would read as a wire: ComfyUI takes a list value wrapped.
        let value = if value.is_array() { json!({ "__value__": value }) } else { value };
        out.insert(full, value);
    }
}

impl<'a> Converter<'a> {
    /// The node and widget input a link inside a subgraph ends at, if it ends at a widget.
    fn target_widget(&self, graph: &Graph<'a>, link: i64) -> Option<(&'a Value, String)> {
        let sub = graph.sub?;
        let l = sub["links"].as_array()?.iter().find(|l| l["id"].as_i64() == Some(link))?;
        let node = graph.node(l["target_id"].as_i64()?)?;
        let input = &node["inputs"][l["target_slot"].as_u64()? as usize];
        input["widget"].is_object().then(|| (node, input["name"].as_str().unwrap_or_default().to_string()))
    }

    /// A node's widgets with their saved values.
    fn widget_values(&self, node: &Value) -> Map<String, Value> {
        let mut values = Map::new();
        let Some(def) = self.info.get(kind(node)) else { return values };
        let inputs = ordered(&def["input"], def.get("input_order"));
        let saved = if node["widgets_values_named"].is_object() { &node["widgets_values_named"] } else { &node["widgets_values"] };
        match saved {
            Value::Object(named) => fill_widgets(&inputs, "", &mut [].iter().peekable(), Some(named), &mut values),
            Value::Array(list) => fill_widgets(&inputs, "", &mut list.iter().peekable(), None, &mut values),
            _ => fill_widgets(&inputs, "", &mut [].iter().peekable(), None, &mut values),
        }
        values
    }

    /// Where the value on `node`'s output `slot` comes from, in `scope`.
    fn source(&mut self, scope: &Scope<'a, '_>, node_id: i64, slot: usize, depth: usize) -> Source {
        if depth > 64 {
            self.problems.push("a loop of wires".into());
            return Source::Nothing;
        }
        if node_id == SUB_IN {
            // From the subgraph's own input: whatever reaches the node it stands in for.
            let Some((instance, outer)) = scope.outer else { return Source::Nothing };
            let name = scope.graph.sub.and_then(|s| s["inputs"][slot]["name"].as_str()).unwrap_or_default().to_string();
            let link = instance["inputs"].as_array().and_then(|i| i.iter().find(|x| x["name"] == name.as_str())).and_then(|x| x["link"].as_i64());
            if let Some(link) = link {
                return self.follow(outer, link, depth + 1);
            }
            if let Some(v) = scope.values.get(&name) {
                return Source::Value(v.clone());
            }
            // Its value is kept by the widget inside it reaches (a promoted widget).
            let links: Vec<i64> = scope.graph.sub.map(|s| s["inputs"][slot]["linkIds"].as_array().into_iter().flatten().filter_map(Value::as_i64).collect()).unwrap_or_default();
            for link in links {
                if let Some((node, input)) = self.target_widget(scope.graph, link)
                    && let Some(v) = self.widget_values(node).get(&input)
                {
                    return Source::Value(v.clone());
                }
            }
            return Source::Nothing;
        }
        let Some(node) = scope.graph.node(node_id) else { return Source::Nothing };
        let k = kind(node);
        if mode(node) == 2 {
            return Source::Nothing;
        }
        if mode(node) == 4 {
            // Bypassed: the first wired input of the same kind passes through.
            let want = node["outputs"][slot]["type"].as_str().unwrap_or("*").to_string();
            let through = node["inputs"].as_array().into_iter().flatten().filter(|i| i["link"].is_i64()).find(|i| want == "*" || i["type"].as_str().is_some_and(|t| t == "*" || t.split(',').any(|t| t == want))).and_then(|i| i["link"].as_i64());
            return match through {
                Some(link) => self.follow(scope, link, depth + 1),
                None => Source::Nothing,
            };
        }
        if k == "Reroute" {
            return match node["inputs"][0]["link"].as_i64() {
                Some(link) => self.follow(scope, link, depth + 1),
                None => Source::Nothing,
            };
        }
        if k == "PrimitiveNode" {
            return node["widgets_values"][0].clone().pipe_some().map(Source::Value).unwrap_or(Source::Nothing);
        }
        if let Some(sub) = self.subgraphs.get(k).copied() {
            // A subgraph's output: whatever reaches its inside's output.
            let graph = Graph::of(sub, Some(sub));
            // The output's own wire, or (when that's missing) the wire that ends at it.
            let named = sub["outputs"][slot]["linkIds"].as_array().into_iter().flatten().filter_map(Value::as_i64).find(|l| graph.links.contains_key(l));
            let ending = || sub["links"].as_array().into_iter().flatten().find(|l| l["target_id"].as_i64() == Some(SUB_OUT) && l["target_slot"].as_u64() == Some(slot as u64)).and_then(|l| l["id"].as_i64());
            let Some(link) = named.or_else(ending) else {
                let what = sub["name"].as_str().unwrap_or(k);
                let out = sub["outputs"][slot]["name"].as_str().unwrap_or("?");
                self.problems.push(format!("inside the subgraph {what}, its output {out} isn't wired to anything"));
                return Source::Nothing;
            };
            let inner = self.scope_inside(scope, node, &graph);
            return self.follow(&inner, link, depth + 1);
        }
        Source::Output(format!("{}{}", scope.prefix, node_id), slot)
    }

    fn follow(&mut self, scope: &Scope<'a, '_>, link: i64, depth: usize) -> Source {
        match scope.graph.links.get(&link).copied() {
            Some(l) => self.source(scope, l.origin, l.origin_slot, depth),
            None => Source::Nothing,
        }
    }

    /// The scope inside a subgraph node, with the values the node keeps for its widgets.
    fn scope_inside<'b>(&self, outer: &'b Scope<'a, 'b>, instance: &'a Value, graph: &'b Graph<'a>) -> Scope<'a, 'b> {
        let sub = graph.sub.expect("a subgraph");
        let mut values = HashMap::new();
        let mut saved = instance["widgets_values"].as_array().into_iter().flatten();
        // The node keeps a value for each of the subgraph's inputs that reaches a widget inside.
        for input in sub["inputs"].as_array().into_iter().flatten() {
            let to_widget = input["linkIds"].as_array().into_iter().flatten().filter_map(Value::as_i64).any(|link| self.target_widget(graph, link).is_some());
            if to_widget && let (Some(name), Some(v)) = (input["name"].as_str(), saved.next()) {
                values.insert(name.to_string(), v.clone());
            }
        }
        Scope { graph, prefix: format!("{}{}:", outer.prefix, id_of(instance)), outer: Some((instance, outer)), values }
    }

    /// Every node of a graph, into the running form.
    fn convert(&mut self, scope: &Scope<'a, '_>, depth: usize) {
        if depth > DEEPEST {
            self.problems.push("subgraphs nested too deep".into());
            return;
        }
        for node in scope.graph.nodes.clone() {
            let k = kind(node);
            if mode(node) == 2 || mode(node) == 4 || k == "Reroute" || k == "PrimitiveNode" || NOTES.contains(&k) {
                continue;
            }
            if let Some(sub) = self.subgraphs.get(k).copied() {
                let graph = Graph::of(sub, Some(sub));
                let inner = self.scope_inside(scope, node, &graph);
                self.convert(&inner, depth + 1);
                continue;
            }
            let Some(def) = self.info.get(k) else {
                // A node that isn't installed, and that nothing needs, is fine to leave out.
                let wired = node["outputs"].as_array().into_iter().flatten().any(|o| o["links"].as_array().is_some_and(|l| !l.is_empty()));
                let shown = node["title"].as_str().unwrap_or(k);
                if wired || node["outputs"].as_array().is_none_or(|o| o.is_empty()) && !k.is_empty() {
                    self.problems.push(format!("the node {shown} ({k}) isn't installed in this ComfyUI"));
                }
                continue;
            };
            let mut values = self.widget_values(node);
            for input in node["inputs"].as_array().into_iter().flatten() {
                let (Some(name), Some(link)) = (input["name"].as_str(), input["link"].as_i64()) else { continue };
                match self.follow(scope, link, 0) {
                    Source::Output(id, slot) => {
                        values.insert(name.to_string(), json!([id, slot]));
                    }
                    Source::Value(v) => {
                        values.insert(name.to_string(), v);
                    }
                    Source::Nothing => {}
                }
            }
            let title = node["title"].as_str().or(def["display_name"].as_str()).unwrap_or(k);
            self.out.insert(format!("{}{}", scope.prefix, id_of(node)), json!({ "class_type": k, "inputs": values, "_meta": { "title": title } }));
        }
    }
}

trait PipeSome {
    fn pipe_some(self) -> Option<Value>;
}

impl PipeSome for Value {
    fn pipe_some(self) -> Option<Value> {
        if self.is_null() { None } else { Some(self) }
    }
}

/// A saved workflow in the form ComfyUI runs, given ComfyUI's `/object_info`; or why it can't be.
pub fn to_run_form(saved: &Value, info: &Value) -> Result<Value, String> {
    if is_run_form(saved) {
        return Ok(saved.clone());
    }
    if !is_editor_form(saved) {
        return Err("that isn't a ComfyUI workflow".into());
    }
    let subgraphs: HashMap<String, &Value> = saved["definitions"]["subgraphs"].as_array().into_iter().flatten().filter_map(|s| Some((s["id"].as_str()?.to_string(), s))).collect();
    let top = Graph::of(saved, None);
    let scope = Scope { graph: &top, prefix: String::new(), outer: None, values: HashMap::new() };
    let mut c = Converter { info, subgraphs, out: Map::new(), problems: Vec::new() };
    c.convert(&scope, 0);
    if let Some(first) = c.problems.first() {
        return Err(first.clone());
    }
    if c.out.is_empty() {
        return Err("the workflow has no nodes ComfyUI runs".into());
    }
    Ok(Value::Object(c.out))
}

/// The values a person would usually change, by "node id.input": prompts, seeds, sizes, steps,
/// models, input pictures. For a bot to see what it can set.
pub fn settable(run: &Value) -> Vec<Value> {
    const WANTED: [&str; 25] = [
        "text", "prompt", "positive", "negative", "seed", "noise_seed", "width", "height", "batch_size", "steps", "cfg", "denoise", "sampler_name", "scheduler", "ckpt_name", "unet_name",
        "lora_name", "strength_model", "image", "video", "filename_prefix", "length", "fps", "guidance", "value",
    ];
    let mut out = Vec::new();
    for (id, node) in run.as_object().into_iter().flatten() {
        for (name, value) in node["inputs"].as_object().into_iter().flatten() {
            if value.is_array() || !WANTED.contains(&name.as_str()) {
                continue;
            }
            out.push(json!({ "set": format!("{id}.{name}"), "node": node["_meta"]["title"], "kind": node["class_type"], "now": value }));
        }
    }
    out.sort_by_key(|v| v["set"].as_str().map(str::to_string));
    out
}

/// Sets values by "node id.input" on a workflow in the running form.
pub fn set_values(run: &mut Value, set: &Map<String, Value>) -> Result<Vec<String>, String> {
    let mut said = Vec::new();
    for (path, value) in set {
        let (id, input) = path.rsplit_once('.').filter(|(id, _)| !id.contains('.') || run.get(*id).is_some()).ok_or_else(|| format!("{path:?}: say which node and input, as \"node id.input\" (from comfy_workflows)"))?;
        // Dynamic inputs have dots of their own ("12.mode.yaw"): the node id is the part before the first dot that names a node.
        let (id, input) = if run.get(id).is_some() { (id.to_string(), input.to_string()) } else {
            let (id, rest) = path.split_once('.').ok_or_else(|| format!("{path:?} isn't \"node id.input\""))?;
            (id.to_string(), rest.to_string())
        };
        let node = run.get_mut(&id).ok_or_else(|| format!("the workflow has no node {id}"))?;
        if node["inputs"].get(&input).is_some_and(Value::is_array) {
            return Err(format!("{path} is wired to another node; set that node's value instead"));
        }
        node["inputs"][&input] = value.clone();
        said.push(format!("{path} = {}", short(value)));
    }
    Ok(said)
}

fn short(v: &Value) -> String {
    let s = match v {
        Value::String(s) => format!("{s:?}"),
        other => other.to_string(),
    };
    if s.chars().count() > 300 { s.chars().take(300).collect::<String>() + "…" } else { s }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info() -> Value {
        json!({
            "KSampler": { "input": { "required": {
                "model": ["MODEL", {}], "seed": ["INT", { "control_after_generate": true }], "steps": ["INT", { "default": 20 }],
                "sampler_name": [["euler", "dpmpp_2m"], {}], "positive": ["CONDITIONING", {}], "latent_image": ["LATENT", {}],
            } }, "input_order": { "required": ["model", "seed", "steps", "sampler_name", "positive", "latent_image"] }, "display_name": "KSampler" },
            "CheckpointLoaderSimple": { "input": { "required": { "ckpt_name": [["a.safetensors"], {}] } }, "display_name": "Load Checkpoint" },
            "CLIPTextEncode": { "input": { "required": { "text": ["STRING", { "multiline": true }], "clip": ["CLIP", {}] } }, "input_order": { "required": ["text", "clip"] } },
            "EmptyLatentImage": { "input": { "required": { "width": ["INT", {}], "height": ["INT", {}] } } },
            "Cam": { "input": { "required": { "mode": ["COMFY_DYNAMICCOMBO_V3", { "options": [
                { "key": "orbit", "inputs": { "required": { "yaw": ["FLOAT", {}], "pitch": ["FLOAT", {}] } } },
                { "key": "look", "inputs": { "required": { "x": ["FLOAT", {}] } } },
            ] }], "fov": ["FLOAT", {}] } }, "input_order": { "required": ["mode", "fov"] } },
        })
    }

    #[test]
    fn widgets_wires_reroutes_and_bypass() {
        let saved = json!({
            "nodes": [
                { "id": 1, "type": "CheckpointLoaderSimple", "widgets_values": ["a.safetensors"], "outputs": [{ "type": "MODEL", "links": [1] }, { "type": "CLIP", "links": [2] }] },
                { "id": 2, "type": "Reroute", "inputs": [{ "name": "", "link": 1 }], "outputs": [{ "type": "*", "links": [3] }] },
                { "id": 3, "type": "KSampler", "widgets_values": [42, "randomize", 30, "dpmpp_2m"],
                  "inputs": [{ "name": "model", "link": 3 }, { "name": "positive", "link": 5 }, { "name": "latent_image", "link": 6 }] },
                { "id": 4, "type": "CLIPTextEncode", "title": "Prompt", "widgets_values": ["a cat"], "inputs": [{ "name": "clip", "link": 2 }], "outputs": [{ "type": "CONDITIONING", "links": [4] }] },
                { "id": 5, "type": "SomeFilter", "mode": 4, "inputs": [{ "name": "c", "type": "CONDITIONING", "link": 4 }], "outputs": [{ "type": "CONDITIONING", "links": [5] }] },
                { "id": 6, "type": "EmptyLatentImage", "widgets_values": [512, 768], "outputs": [{ "type": "LATENT", "links": [6] }] },
                { "id": 7, "type": "MarkdownNote", "widgets_values": ["notes"] },
            ],
            "links": [[1, 1, 0, 2, 0, "MODEL"], [2, 1, 1, 4, 1, "CLIP"], [3, 2, 0, 3, 0, "MODEL"], [4, 4, 0, 5, 0, "CONDITIONING"], [5, 5, 0, 3, 4, "CONDITIONING"], [6, 6, 0, 3, 5, "LATENT"]],
        });
        let run = to_run_form(&saved, &info()).unwrap();
        assert_eq!(run["3"]["inputs"], json!({ "seed": 42, "steps": 30, "sampler_name": "dpmpp_2m", "model": ["1", 0], "positive": ["4", 0], "latent_image": ["6", 0] }));
        assert_eq!(run["4"]["inputs"], json!({ "text": "a cat", "clip": ["1", 1] }));
        assert_eq!(run["4"]["_meta"]["title"], "Prompt");
        assert!(run.get("5").is_none() && run.get("7").is_none() && run.get("2").is_none());
        let all = settable(&run);
        assert!(all.iter().any(|s| s["set"] == "4.text" && s["now"] == "a cat"), "{all:?}");
        let mut run = run;
        let said = set_values(&mut run, json!({ "4.text": "a dog", "3.seed": 7 }).as_object().unwrap()).unwrap();
        assert_eq!(said.len(), 2);
        assert_eq!(run["4"]["inputs"]["text"], "a dog");
        assert!(set_values(&mut run, json!({ "3.model": "x" }).as_object().unwrap()).unwrap_err().contains("wired"));
    }

    #[test]
    fn dynamic_inputs_and_subgraphs() {
        let saved = json!({
            "nodes": [
                { "id": 9, "type": "Cam", "widgets_values": ["orbit", 10.0, 20.0, 35.0] },
                { "id": 20, "type": "sub-1", "widgets_values": ["inside text"], "inputs": [{ "name": "clip", "link": 30 }], "outputs": [{ "type": "CONDITIONING", "links": [] }] },
                { "id": 21, "type": "CheckpointLoaderSimple", "widgets_values": ["a.safetensors"], "outputs": [{ "type": "MODEL", "links": [] }, { "type": "CLIP", "links": [30] }] },
            ],
            "links": [[30, 21, 1, 20, 0, "CLIP"]],
            "definitions": { "subgraphs": [{
                "id": "sub-1",
                "inputs": [{ "name": "clip", "type": "CLIP", "linkIds": [1] }, { "name": "text", "type": "STRING", "linkIds": [2] }],
                "outputs": [{ "name": "c", "type": "CONDITIONING", "linkIds": [3] }],
                "nodes": [{ "id": 5, "type": "CLIPTextEncode", "widgets_values": ["stale"], "inputs": [{ "name": "text", "widget": { "name": "text" }, "link": 2 }, { "name": "clip", "link": 1 }] }],
                "links": [
                    { "id": 1, "origin_id": -10, "origin_slot": 0, "target_id": 5, "target_slot": 1, "type": "CLIP" },
                    { "id": 2, "origin_id": -10, "origin_slot": 1, "target_id": 5, "target_slot": 0, "type": "STRING" },
                    { "id": 3, "origin_id": 5, "origin_slot": 0, "target_id": -20, "target_slot": 0, "type": "CONDITIONING" },
                ],
            }] },
        });
        let run = to_run_form(&saved, &info()).unwrap();
        assert_eq!(run["9"]["inputs"], json!({ "mode": "orbit", "mode.yaw": 10.0, "mode.pitch": 20.0, "fov": 35.0 }));
        assert_eq!(run["20:5"]["inputs"], json!({ "text": "inside text", "clip": ["21", 1] }));
    }

    #[test]
    fn a_missing_node_that_matters_is_named() {
        let saved = json!({ "nodes": [{ "id": 1, "type": "Fancy", "title": "Fancy thing", "outputs": [{ "type": "IMAGE", "links": [1] }] }], "links": [] });
        assert!(to_run_form(&saved, &info()).unwrap_err().contains("Fancy thing (Fancy) isn't installed"));
    }
}
