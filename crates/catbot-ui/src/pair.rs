//! The phone app's first screen: pairing with Catbot at home, by scanning the QR code the
//! home screen shows (Devices away from home → Pair a phone) or typing its address and code.
//!
//! Scanning opens the camera in the app's own page: the browser's BarcodeDetector where it
//! has one, else jsQR (Apache-2.0, `assets/jsQR.js`, see NOTICE) reading frames off a canvas.

use dioxus::prelude::*;
use serde_json::{Value, json};

use crate::link::{self, Link};
use crate::mascots::BotMark;

const JSQR: Asset = asset!("/assets/jsQR.js");

const FIELD: &str = "w-full rounded-xl border border-hairline/60 bg-inset px-3 py-2.5 text-[15px] text-ink outline-none focus:border-accent/60";

/// Opens the camera full screen until it reads a QR code (or the person cancels), then sends
/// back the code's text, "cancel:", or "error:<why>".
fn scanner_script() -> String {
    format!(
        r#"
const jsqrUrl = "{JSQR}";
const result = await new Promise((resolve) => {{
  const cover = document.createElement("div");
  cover.style.cssText = "position:fixed;inset:0;z-index:9999;background:#000;display:flex;flex-direction:column;align-items:center;justify-content:center;gap:16px";
  const video = document.createElement("video");
  video.setAttribute("playsinline", "");
  video.muted = true;
  video.style.cssText = "width:100%;max-height:75vh;object-fit:cover";
  const hint = document.createElement("p");
  hint.textContent = "Point the camera at the QR code on your computer";
  hint.style.cssText = "color:#fff;font:15px system-ui;margin:0 16px;text-align:center";
  const cancel = document.createElement("button");
  cancel.textContent = "Cancel";
  cancel.style.cssText = "color:#fff;background:rgba(255,255,255,.15);border:0;border-radius:999px;padding:10px 22px;font:600 15px system-ui";
  cover.append(video, hint, cancel);
  document.body.append(cover);
  let stream = null, done = false;
  const finish = (text) => {{
    if (done) return;
    done = true;
    if (stream) stream.getTracks().forEach((t) => t.stop());
    cover.remove();
    resolve(text);
  }};
  cancel.onclick = () => finish("cancel:");
  (async () => {{
    try {{
      stream = await navigator.mediaDevices.getUserMedia({{ video: {{ facingMode: "environment" }}, audio: false }});
      video.srcObject = stream;
      await video.play();
      let detector = null;
      if ("BarcodeDetector" in window) {{
        try {{ detector = new BarcodeDetector({{ formats: ["qr_code"] }}); }} catch (_) {{ detector = null; }}
      }}
      if (!detector && !window.jsQR) {{
        await new Promise((ok, fail) => {{
          const s = document.createElement("script");
          s.src = jsqrUrl; s.onload = ok; s.onerror = () => fail(new Error("the QR reader didn't load"));
          document.head.append(s);
        }});
      }}
      const canvas = document.createElement("canvas");
      const ctx = canvas.getContext("2d", {{ willReadFrequently: true }});
      const look = async () => {{
        if (done) return;
        try {{
          if (video.videoWidth > 0) {{
            if (detector) {{
              const codes = await detector.detect(video);
              if (codes.length) return finish(codes[0].rawValue);
            }} else {{
              canvas.width = video.videoWidth; canvas.height = video.videoHeight;
              ctx.drawImage(video, 0, 0);
              const frame = ctx.getImageData(0, 0, canvas.width, canvas.height);
              const code = window.jsQR(frame.data, frame.width, frame.height, {{ inversionAttempts: "dontInvert" }});
              if (code && code.data) return finish(code.data);
            }}
          }}
        }} catch (_) {{}}
        setTimeout(look, 150);
      }};
      look();
    }} catch (e) {{
      finish("error:" + (e && e.message ? e.message : e));
    }}
  }})();
}});
dioxus.send(result);
"#
    )
}

/// Trades a code for this device's key at Catbot's remote door.
async fn pair(url: &str, code: &str, name: &str) -> Result<Link, String> {
    let url = url.trim().trim_end_matches('/').to_string();
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("The address should start with https://".into());
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let response = crate::api::client()
            .post(format!("{url}/api/pair"))
            .json(&json!({ "code": code, "name": name }))
            .send()
            .await
            .map_err(|e| format!("Couldn't reach Catbot at {url} ({e})"))?;
        let status = response.status().as_u16();
        let body: Value = response.json().await.unwrap_or(Value::Null);
        if status != 201 {
            return Err(body["error"].as_str().map(str::to_string).unwrap_or_else(|| format!("Catbot answered {status}")));
        }
        let key = body["key"].as_str().ok_or("Catbot didn't send a key")?.to_string();
        let name = body["device"]["name"].as_str().unwrap_or(name).to_string();
        Ok(Link { url, key, name })
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (code, name);
        Err("Pairing is for the phone app".into())
    }
}

#[component]
pub fn PairScreen(
    on_paired: EventHandler<()>,
    removed: bool,
    /// Pairing a computer (the Windows app reaching home) rather than the phone: typed, not
    /// scanned, over the app, with a Cancel.
    #[props(default)]
    computer: bool,
    on_cancel: Option<EventHandler<()>>,
) -> Element {
    let device = if computer { "Work PC" } else { "Phone" };
    let mut url = use_signal(String::new);
    let mut code = use_signal(String::new);
    let mut name = use_signal(|| device.to_string());
    let mut busy = use_signal(|| false);
    let mut problem = use_signal(|| None::<String>);

    let mut go = move |address: String, typed: String| {
        if busy() {
            return;
        }
        busy.set(true);
        problem.set(None);
        let device = name.peek().trim().to_string();
        spawn(async move {
            let device = if device.is_empty() { if computer { "Work PC".to_string() } else { "Phone".to_string() } } else { device };
            match pair(&address, &typed, &device).await {
                Ok(paired) => match link::save(paired) {
                    Ok(()) => on_paired.call(()),
                    Err(err) => problem.set(Some(err)),
                },
                Err(err) => problem.set(Some(err)),
            }
            busy.set(false);
        });
    };

    let scan = move |_| {
        problem.set(None);
        spawn(async move {
            let mut eval = document::eval(&scanner_script());
            let Ok(text) = eval.recv::<String>().await else {
                problem.set(Some("The camera didn't open.".into()));
                return;
            };
            if text == "cancel:" {
                return;
            }
            if let Some(why) = text.strip_prefix("error:") {
                problem.set(Some(format!("The camera didn't open ({why}). You can type the address and code instead.")));
                return;
            }
            match link::parse_invite(&text) {
                Some((address, found)) => {
                    url.set(address.clone());
                    code.set(found.clone());
                    // Fill the boxes too, so they show what was read.
                    document::eval(&format!(
                        r#"const a = document.getElementById("pair-url"); if (a) a.value = {}; const c = document.getElementById("pair-code"); if (c) c.value = {};"#,
                        serde_json::to_string(&address).unwrap_or_default(),
                        serde_json::to_string(&found).unwrap_or_default()
                    ));
                    go(address, found);
                }
                None => problem.set(Some("That QR code isn't a Catbot pairing code.".into())),
            }
        });
    };
    let mut send = go;
    let submit = move |_| send(url.peek().clone(), code.peek().clone());

    rsx! {
        div { "data-pair-screen": true, class: if computer { "fixed inset-0 z-50 flex flex-col items-center justify-center gap-5 overflow-y-auto bg-app px-6 py-10 text-ink" } else { "flex min-h-screen flex-col items-center justify-center gap-5 bg-app px-6 py-10 text-ink" },
            BotMark { color: "teal".to_string(), initial: "C".to_string(), size: 88, mark: "cat-visor".to_string() }
            div { class: "flex flex-col items-center gap-2 text-center",
                h1 { class: "text-[22px] font-semibold", if computer { "Pair with Catbot at home" } else { "Pair with Catbot" } }
                p { class: "max-w-[340px] text-[14px] leading-[20px] text-ink-secondary",
                    if computer {
                        "On your home computer, open Catbot, click the phone button at the top of the sidebar, then Pair a phone. Type the address and code it shows."
                    } else {
                        "On your computer, open Catbot, click the phone button at the top of the sidebar, then Pair a phone."
                    }
                }
                if removed {
                    p { role: "alert", class: "max-w-[340px] text-[14px] font-medium text-warning", "This phone was removed at home. Pair it again to carry on." }
                }
            }
            if !computer { button {
                r#type: "button",
                "data-scan": true,
                disabled: busy(),
                class: "w-full max-w-[340px] rounded-full bg-accent px-4 py-3 text-[16px] font-semibold text-white hover:opacity-90 disabled:opacity-50",
                onclick: scan,
                "Scan the QR code"
            }
            div { class: "flex w-full max-w-[340px] items-center gap-3 text-[12px] text-ink-secondary",
                span { class: "h-px flex-1 bg-hairline" }
                "or type it"
                span { class: "h-px flex-1 bg-hairline" }
            } }
            div { class: "flex w-full max-w-[340px] flex-col gap-3",
                label { class: "flex flex-col gap-1 text-[13px] text-ink-secondary",
                    "Catbot's address"
                    input { id: "pair-url", r#type: "url", placeholder: "https://…ts.net", autocomplete: "off", class: FIELD, oninput: move |e| url.set(e.value()) }
                }
                label { class: "flex flex-col gap-1 text-[13px] text-ink-secondary",
                    "Code"
                    input { id: "pair-code", placeholder: "ABCDE-FGHJK", autocomplete: "off", autocapitalize: "characters", class: "{FIELD} font-mono tracking-[0.1em]", oninput: move |e| code.set(e.value()) }
                }
                label { class: "flex flex-col gap-1 text-[13px] text-ink-secondary",
                    if computer { "This computer's name" } else { "This phone's name" }
                    input { id: "pair-name", initial_value: "{device}", autocomplete: "off", class: FIELD, oninput: move |e| name.set(e.value()) }
                }
                button {
                    r#type: "button",
                    "data-pair": true,
                    disabled: busy(),
                    class: "mt-1 rounded-full bg-raised px-4 py-2.5 text-[15px] font-semibold text-ink hover:bg-control disabled:opacity-50",
                    onclick: submit,
                    if busy() { "Pairing…" } else { "Pair" }
                }
                if let Some(cancel) = on_cancel {
                    button {
                        r#type: "button",
                        "data-pair-cancel": true,
                        class: "rounded-full px-4 py-2 text-[14px] font-medium text-ink-secondary hover:bg-raised",
                        onclick: move |_| cancel.call(()),
                        "Cancel"
                    }
                }
            }
            if let Some(err) = problem() {
                p { role: "alert", "data-pair-problem": true, class: "max-w-[340px] text-center text-[14px] text-danger", "{err}" }
            }
        }
    }
}
