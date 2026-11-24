//! How paired phones reach this computer (stage 19.7.1), after MausBot's "How phones reach"
//! settings (`PhoneDevicesSection.tsx`).
//!
//! Moo's choices (2026-11-24):
//! * **Same Wi-Fi**: a switch, off until he turns it on. On, the remote door also listens on
//!   this computer's home-network address (plain http, port: the remote door's + 1, or
//!   `CATBOT_LAN_PORT`), and pairing can offer that address. A device still needs a pairing code
//!   and its key; only use it on a trusted network.
//! * **Keep this computer awake** while a way in is on (MausBot's way), as a switch: the system
//!   is kept from sleeping (Linux `systemd-inhibit`, macOS `caffeinate`, Windows
//!   `SetThreadExecutionState` through PowerShell), and lets go when it's turned off.
//! * **Check connection**: whether the remote door answers here, whether the public address
//!   reaches it, and whether the Wi-Fi address does.
//!
//! Kept in the settings under `reach`.
//!
//! * `GET /api/devices/reach`   `{ sameWifi, lanUrl, keepAwake, awake, awakeProblem, wifiProblem }`
//! * `PUT /api/devices/reach`   `{ sameWifi?, keepAwake? }`
//! * `POST /api/devices/check`  `{ checks: [{ id, ok, line }] }`

use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hyper::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Answer, App, bad, fail};

const KEY: &str = "reach";

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Reach {
    pub same_wifi: bool,
    pub keep_awake: bool,
}

pub fn load(app: &App) -> Reach {
    app.with_store(|store| store.setting(KEY)).flatten().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
}

/// The Wi-Fi door while it's open: its address, and how to close it.
static WIFI: Mutex<Option<(SocketAddr, tokio::sync::oneshot::Sender<()>)>> = Mutex::new(None);
static WIFI_PROBLEM: Mutex<Option<String>> = Mutex::new(None);
/// What keeps the computer awake, while it runs.
static AWAKE: Mutex<Option<tokio::process::Child>> = Mutex::new(None);
static AWAKE_PROBLEM: Mutex<Option<String>> = Mutex::new(None);

/// This computer's address on its home network: the one a route to the internet goes out by
/// (a UDP socket "connected" outward sends nothing).
pub fn lan_ip() -> Option<IpAddr> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:80").ok()?;
    let ip = socket.local_addr().ok()?.ip();
    (!ip.is_loopback() && !ip.is_unspecified()).then_some(ip)
}

fn lan_port(app: &App) -> Option<u16> {
    if let Some(port) = std::env::var("CATBOT_LAN_PORT").ok().and_then(|p| p.trim().parse().ok()) {
        return Some(port);
    }
    app.remote.get().map(|addr| addr.port().saturating_add(1))
}

/// The Wi-Fi address paired phones would use, while the Wi-Fi door is open.
pub fn lan_url() -> Option<String> {
    let open = WIFI.lock().ok()?.as_ref().map(|(addr, _)| *addr)?;
    let ip = lan_ip()?;
    Some(format!("http://{ip}:{}", open.port()))
}

/// Opens or closes the Wi-Fi door to match the setting.
pub async fn set_wifi(app: &Arc<App>, on: bool) {
    let open = WIFI.lock().map(|w| w.is_some()).unwrap_or(false);
    if !on {
        if let Some((addr, stop)) = WIFI.lock().ok().and_then(|mut w| w.take()) {
            let _ = stop.send(());
            eprintln!("catbot: closed the same-Wi-Fi door on {addr}");
        }
        *WIFI_PROBLEM.lock().unwrap_or_else(|e| e.into_inner()) = None;
        return;
    }
    if open {
        return;
    }
    let problem = match (app.remote.get(), lan_port(app)) {
        (None, _) | (_, None) => Some("the remote door is off (catbot serve --remote off)".to_string()),
        (Some(_), Some(port)) => {
            let addr = SocketAddr::from(([0, 0, 0, 0], port));
            match tokio::net::TcpListener::bind(addr).await {
                Ok(listener) => {
                    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
                    let router = super::remote::router(app.clone());
                    tokio::spawn(async move {
                        let shutdown = async move {
                            let _ = stopped.await;
                        };
                        if let Err(err) = axum::serve(listener, router).with_graceful_shutdown(shutdown).await {
                            eprintln!("catbot: the same-Wi-Fi door stopped: {err}");
                        }
                    });
                    eprintln!("catbot: same-Wi-Fi door for paired devices: http://{}:{port}", lan_ip().map(|ip| ip.to_string()).unwrap_or_else(|| "0.0.0.0".into()));
                    *WIFI.lock().unwrap_or_else(|e| e.into_inner()) = Some((addr, stop));
                    None
                }
                Err(err) => Some(format!("couldn't listen on port {port}: {err}")),
            }
        }
    };
    *WIFI_PROBLEM.lock().unwrap_or_else(|e| e.into_inner()) = problem;
}

/// The program that keeps this computer awake, for this system.
fn awake_command() -> tokio::process::Command {
    if cfg!(target_os = "windows") {
        let script = "$s='[DllImport(\"kernel32.dll\")] public static extern uint SetThreadExecutionState(uint e);'; \
                      $k=Add-Type -MemberDefinition $s -Name Awake -Namespace Catbot -PassThru; \
                      while ($true) { [void]$k::SetThreadExecutionState(0x80000001); Start-Sleep -Seconds 30 }";
        let mut command = tokio::process::Command::new("powershell");
        command.args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", script]);
        command
    } else if cfg!(target_os = "macos") {
        let mut command = tokio::process::Command::new("caffeinate");
        command.args(["-i", "-s"]);
        command
    } else {
        let mut command = tokio::process::Command::new("systemd-inhibit");
        command.args(["--what=sleep:idle", "--who=Catbot", "--why=Paired phones can reach Catbot", "--mode=block", "sleep", "infinity"]);
        command
    }
}

/// Keeps the computer awake, or lets it sleep, to match the setting.
pub fn set_awake(app: &App, on: bool) {
    let on = on && app.remote.get().is_some();
    let mut awake = AWAKE.lock().unwrap_or_else(|e| e.into_inner());
    // Still running? (It may have been stopped from outside.)
    if let Some(child) = awake.as_mut()
        && child.try_wait().ok().flatten().is_some()
    {
        *awake = None;
    }
    let mut problem = AWAKE_PROBLEM.lock().unwrap_or_else(|e| e.into_inner());
    match (on, awake.is_some()) {
        (true, false) => {
            let mut command = awake_command();
            command.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).kill_on_drop(true);
            match command.spawn() {
                Ok(child) => {
                    eprintln!("catbot: keeping this computer awake while phones can reach it");
                    *awake = Some(child);
                    *problem = None;
                }
                Err(err) => *problem = Some(format!("couldn't keep this computer awake: {err}")),
            }
        }
        (false, true) => {
            if let Some(mut child) = awake.take() {
                let _ = child.start_kill();
                eprintln!("catbot: this computer may sleep again");
            }
            *problem = None;
        }
        (false, false) => *problem = None,
        (true, true) => {}
    }
}

/// At start: the Wi-Fi door and staying awake, as last set.
pub async fn start(app: &Arc<App>) {
    let reach = load(app);
    if reach.same_wifi {
        set_wifi(app, true).await;
    }
    set_awake(app, reach.keep_awake);
}

fn wire(app: &App) -> Value {
    let reach = load(app);
    let awake = AWAKE.lock().map(|a| a.is_some()).unwrap_or(false);
    json!({
        "sameWifi": reach.same_wifi,
        "lanUrl": lan_url(),
        "wifiProblem": *WIFI_PROBLEM.lock().unwrap_or_else(|e| e.into_inner()),
        "keepAwake": reach.keep_awake,
        "awake": awake,
        "awakeProblem": *AWAKE_PROBLEM.lock().unwrap_or_else(|e| e.into_inner()),
        "remote": app.remote.get().is_some(),
    })
}

pub fn get(app: &App) -> Answer {
    Ok((StatusCode::OK, wire(app)))
}

pub async fn put(app: &Arc<App>, body: &Value) -> Answer {
    let mut reach = load(app);
    if let Some(on) = body.get("sameWifi") {
        reach.same_wifi = on.as_bool().ok_or_else(|| bad("sameWifi is true or false"))?;
    }
    if let Some(on) = body.get("keepAwake") {
        reach.keep_awake = on.as_bool().ok_or_else(|| bad("keepAwake is true or false"))?;
    }
    app.try_store(|store| store.set_setting(KEY, &json!(reach))).map_err(fail)?;
    set_wifi(app, reach.same_wifi).await;
    set_awake(app, reach.keep_awake);
    app.hub.send(json!({ "kind": "devices" }));
    get(app)
}

/// Whether an address answers like Catbot's remote door: an unpaired request is refused with
/// 401 (anything else means something else is there).
async fn answers(url: &str) -> Result<(), String> {
    let client = reqwest::Client::builder().timeout(Duration::from_secs(8)).build().map_err(|e| e.to_string())?;
    match client.get(format!("{}/api/device", url.trim_end_matches('/'))).send().await {
        Ok(reply) if reply.status() == reqwest::StatusCode::UNAUTHORIZED => Ok(()),
        Ok(reply) => Err(format!("something else answered there ({})", reply.status())),
        Err(err) if err.is_timeout() => Err("no answer in 8 seconds".into()),
        Err(err) if err.is_connect() => Err("couldn't connect".into()),
        Err(err) => Err(err.to_string()),
    }
}

/// Checks each way in: the door here, the public address, the Wi-Fi address.
pub async fn check(app: &Arc<App>) -> Answer {
    let mut checks = Vec::new();
    let Some(door) = app.remote.get().copied() else {
        checks.push(json!({ "id": "door", "ok": false, "line": "The remote door is off: Catbot was started with --remote off." }));
        return Ok((StatusCode::OK, json!({ "checks": checks })));
    };
    let here = format!("http://127.0.0.1:{}", door.port());
    checks.push(match answers(&here).await {
        Ok(()) => json!({ "id": "door", "ok": true, "line": format!("The remote door answers on this computer (port {}).", door.port()) }),
        Err(why) => json!({ "id": "door", "ok": false, "line": format!("The remote door doesn't answer on this computer: {why}.") }),
    });
    checks.push(match super::devices::public_url().await {
        Some(url) => match answers(&url).await {
            Ok(()) => json!({ "id": "public", "ok": true, "line": format!("{url} reaches it, so phones can from anywhere.") }),
            Err(why) => json!({ "id": "public", "ok": false, "line": format!("{url} doesn't reach it: {why}. Is Tailscale Funnel on? Run scripts/remote-setup.sh.") }),
        },
        None => json!({ "id": "public", "ok": false, "line": "No public address: Tailscale isn't set up. Run scripts/remote-setup.sh once." }),
    });
    if load(app).same_wifi {
        checks.push(match lan_url() {
            Some(url) => match answers(&url).await {
                Ok(()) => json!({ "id": "wifi", "ok": true, "line": format!("{url} reaches it on this Wi-Fi.") }),
                Err(why) => json!({ "id": "wifi", "ok": false, "line": format!("{url} doesn't reach it: {why}. A firewall may be blocking the port.") }),
            },
            None => json!({ "id": "wifi", "ok": false, "line": "Same Wi-Fi is on but its door isn't open (or this computer has no home-network address)." }),
        });
    }
    Ok((StatusCode::OK, json!({ "checks": checks })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reach_reads_old_and_partial_settings() {
        let reach: Reach = serde_json::from_value(json!({ "sameWifi": true })).unwrap();
        assert_eq!(reach, Reach { same_wifi: true, keep_awake: false });
        assert_eq!(Reach::default(), Reach { same_wifi: false, keep_awake: false });
    }
}
