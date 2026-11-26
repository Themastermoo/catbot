//! "Continue with ChatGPT" (stage 29.10): a ChatGPT plan used by Catbot's own bots, the way
//! OpenMausBot does it (its `server/drivers/chatgpt-plan-auth.ts`, Apache-2.0).
//!
//! Catbot registers itself with OpenAI as an agent ("catbot — An agent you connected to
//! ChatGPT" in ChatGPT's settings) through OAuth with PKCE at `auth.openai.com`: the first
//! sign-in asks for the `dynamic_agent_client` and OpenAI hands back this computer's own client
//! id, kept for the next sign-ins. The browser comes back to a one-time listener on 127.0.0.1;
//! the code is traded for tokens, the identity token checked against OpenAI's keys, and the
//! tokens kept in a file only this user can read. A bot on "ChatGPT (plan)" runs on Codex with
//! the access token, renewed before each turn.
//!
//! Nothing here reaches the interface but the sign-in page's address and safe status.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant},
};

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::Digest as _;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

const ISSUER: &str = "https://auth.openai.com";
const RESOURCE: &str = "https://api.openai.com/v1";
/// The scope that lets an agent use the person's plan.
pub const DIRECT_SCOPE: &str = "chatgpt.tokens.use.direct";
const SCOPES: &str = "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
/// The name ChatGPT shows for Catbot in its list of connected agents.
const AGENT_NAME: &str = "catbot";
/// How long a sign-in waits for the browser.
const LIFETIME: Duration = Duration::from_secs(15 * 60);
pub const SIGN_IN: &str = "Continue with ChatGPT in Settings › AI to use your ChatGPT plan.";
pub const CONSENT: &str = "ChatGPT plan usage isn't allowed yet. Continue with ChatGPT again and allow it.";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
struct Tokens {
    access: String,
    refresh: String,
    id: String,
    scopes: Vec<String>,
    /// Unix milliseconds.
    expires_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    earliest_refresh_at: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
struct Credentials {
    client_id: String,
    subject: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    email: Option<String>,
    host_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tokens: Option<Tokens>,
}

/// A sign-in under way, or how the last one ended.
#[derive(Clone, Debug)]
struct Flow {
    id: String,
    /// "waiting", "succeeded", "failed", "expired", "cancelled".
    phase: &'static str,
    url: Option<String>,
    message: Option<String>,
    state: String,
    nonce: String,
    verifier: String,
    redirect_uri: String,
    previous: Option<Credentials>,
    client_id: Option<String>,
    host_id: String,
    started: Instant,
}

/// The ChatGPT plan's sign-in and tokens, for this computer.
pub struct Plan {
    dir: PathBuf,
    issuer: String,
    resource: String,
    flow: Mutex<Option<Flow>>,
    /// A client OpenAI made for a sign-in that didn't finish: used again, not a new one each try.
    retry_client: Mutex<Option<String>>,
    /// One renewal at a time: a refresh token is used once.
    renewing: tokio::sync::Mutex<()>,
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn random_b64() -> String {
    let mut bytes = Vec::with_capacity(32);
    bytes.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    bytes.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn private_dir(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        use std::io::Write;
        let mut file = options.open(&tmp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
    }
    std::fs::rename(tmp, path)
}

impl Plan {
    /// `dir`: a private folder of Catbot's for this account. `CATBOT_CHATGPT_AUTH` (a loopback
    /// address) stands in for OpenAI in tests.
    pub fn new(dir: PathBuf) -> Self {
        let (issuer, resource) = match std::env::var("CATBOT_CHATGPT_AUTH").ok().filter(|u| u.starts_with("http://127.0.0.1:")) {
            Some(test) => (test.trim_end_matches('/').to_string(), format!("{}/v1", test.trim_end_matches('/'))),
            None => (ISSUER.to_string(), RESOURCE.to_string()),
        };
        Plan { dir, issuer, resource, flow: Mutex::new(None), retry_client: Mutex::new(None), renewing: tokio::sync::Mutex::new(()) }
    }

    /// The API address bots use with the token.
    pub fn resource(&self) -> &str {
        &self.resource
    }

    fn read(&self) -> Result<Option<Credentials>, String> {
        match std::fs::read_to_string(self.dir.join("credentials.json")) {
            Ok(text) => serde_json::from_str(&text).map(Some).map_err(|_| "The saved ChatGPT sign-in can't be read. Continue with ChatGPT again.".to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err("The saved ChatGPT sign-in can't be read safely.".into()),
        }
    }

    fn save(&self, record: &Credentials) -> Result<(), String> {
        private_dir(&self.dir).map_err(|e| e.to_string())?;
        write_private(&self.dir.join("credentials.json"), &serde_json::to_string(record).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
    }

    /// This computer's own id with OpenAI, kept beside the account folder.
    fn host_id(&self) -> Result<String, String> {
        let parent = self.dir.parent().unwrap_or(&self.dir).to_path_buf();
        private_dir(&parent).map_err(|e| e.to_string())?;
        let path = parent.join("host-id.json");
        if let Ok(text) = std::fs::read_to_string(&path) {
            let value: Value = serde_json::from_str(&text).map_err(|_| "The saved ChatGPT host id is invalid.")?;
            return value["hostId"].as_str().filter(|h| h.starts_with("urn:uuid:")).map(str::to_string).ok_or("The saved ChatGPT host id is invalid.".into());
        }
        let id = format!("urn:uuid:{}", uuid::Uuid::new_v4());
        write_private(&path, &json!({ "hostId": id }).to_string()).map_err(|e| e.to_string())?;
        Ok(id)
    }

    /// What the interface may know: signed in or not, as whom, why not, and a sign-in under way.
    pub fn snapshot(&self) -> Value {
        let record = self.read();
        let flow = self.flow.lock().expect("flow lock").clone();
        let sign_in = flow.map(|f| json!({ "id": f.id, "phase": f.phase, "url": f.url, "message": f.message }));
        match record {
            Ok(Some(r)) => {
                let allowed = r.tokens.as_ref().is_some_and(|t| t.scopes.iter().any(|s| s == DIRECT_SCOPE));
                json!({
                    "signedIn": allowed,
                    "email": r.email,
                    "reason": if r.tokens.is_none() { Some(SIGN_IN) } else if !allowed { Some(CONSENT) } else { None },
                    "signIn": sign_in,
                })
            }
            Ok(None) => json!({ "signedIn": false, "reason": SIGN_IN, "signIn": sign_in }),
            Err(problem) => json!({ "signedIn": false, "reason": problem, "signIn": sign_in }),
        }
    }

    pub fn signed_in(&self) -> bool {
        self.snapshot()["signedIn"] == true
    }

    /// Starts a sign-in: listens for the browser on 127.0.0.1 and gives the page to open.
    /// `done` hears when the sign-in has ended, whichever way.
    pub async fn start(self: &std::sync::Arc<Self>, done: impl Fn() + Send + Sync + 'static) -> Result<String, String> {
        if let Some(f) = self.flow.lock().expect("flow lock").as_ref().filter(|f| f.phase == "waiting" && f.started.elapsed() < LIFETIME) {
            return Ok(f.url.clone().unwrap_or_default());
        }
        let previous = self.read()?;
        let client_id = previous.as_ref().map(|p| p.client_id.clone()).or_else(|| self.retry_client.lock().expect("retry lock").clone());
        let host_id = self.host_id()?;
        if previous.as_ref().is_some_and(|p| p.host_id != host_id) {
            return Err("This ChatGPT sign-in belongs to another computer.".into());
        }
        let listener = TcpListener::bind("127.0.0.1:0").await.map_err(|e| format!("couldn't listen for the sign-in: {e}"))?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let redirect_uri = format!("http://127.0.0.1:{port}/auth/callback");
        let (state, nonce, verifier) = (random_b64(), random_b64(), random_b64());
        let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(verifier.as_bytes()));
        let mut url = reqwest::Url::parse(&format!("{}/api/accounts/authorize", self.issuer)).map_err(|e| e.to_string())?;
        {
            let mut q = url.query_pairs_mut();
            q.append_pair("client_id", client_id.as_deref().unwrap_or("dynamic_agent_client"));
            q.append_pair("ext_agent_host_id", &host_id);
            if client_id.is_none() {
                q.append_pair("agent_name_hint", AGENT_NAME);
            }
            if let Some(email) = previous.as_ref().and_then(|p| p.email.as_deref()) {
                q.append_pair("login_hint", email);
            }
            if previous.as_ref().and_then(|p| p.tokens.as_ref()).is_some_and(|t| !t.scopes.iter().any(|s| s == DIRECT_SCOPE)) {
                q.append_pair("prompt", "consent");
            }
            q.append_pair("response_type", "code");
            q.append_pair("redirect_uri", &redirect_uri);
            q.append_pair("scope", SCOPES);
            q.append_pair("resource", &self.resource);
            q.append_pair("state", &state);
            q.append_pair("nonce", &nonce);
            q.append_pair("code_challenge_method", "S256");
            q.append_pair("code_challenge", &challenge);
        }
        let flow = Flow {
            id: uuid::Uuid::new_v4().to_string(),
            phase: "waiting",
            url: Some(url.to_string()),
            message: None,
            state,
            nonce,
            verifier,
            redirect_uri,
            previous,
            client_id,
            host_id,
            started: Instant::now(),
        };
        let flow_id = flow.id.clone();
        *self.flow.lock().expect("flow lock") = Some(flow);
        let plan = self.clone();
        tokio::spawn(async move {
            let answer = tokio::time::timeout(LIFETIME, plan.callback(listener, port)).await;
            match answer {
                Ok(Some(params)) => plan.complete(&flow_id, params).await,
                Ok(None) => plan.finish(&flow_id, "failed", Some("The sign-in didn't come back. Start again.")),
                Err(_) => plan.finish(&flow_id, "expired", Some("The ChatGPT sign-in expired. Start again.")),
            }
            done();
        });
        Ok(url.to_string())
    }

    /// Waits for the browser's one visit to /auth/callback with the right state: its query.
    async fn callback(&self, listener: TcpListener, port: u16) -> Option<HashMap<String, String>> {
        loop {
            let (mut socket, _) = listener.accept().await.ok()?;
            let mut buf = vec![0u8; 8192];
            let mut got = 0;
            while got < buf.len() {
                let Ok(n) = tokio::time::timeout(Duration::from_secs(10), socket.read(&mut buf[got..])).await.unwrap_or(Ok(0)) else { break };
                if n == 0 {
                    break;
                }
                got += n;
                if buf[..got].windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let request = String::from_utf8_lossy(&buf[..got]).to_string();
            let mut lines = request.lines();
            let first = lines.next().unwrap_or_default();
            let host_ok = lines.any(|l| l.to_ascii_lowercase().strip_prefix("host:").is_some_and(|h| h.trim() == format!("127.0.0.1:{port}")));
            let target = first.strip_prefix("GET ").and_then(|r| r.split(' ').next()).unwrap_or_default();
            let parsed = reqwest::Url::parse(&format!("http://127.0.0.1:{port}{target}")).ok();
            let mut params: HashMap<String, String> = HashMap::new();
            let mut doubled = false;
            if let Some(u) = &parsed {
                for (k, v) in u.query_pairs() {
                    doubled |= params.insert(k.into_owned(), v.into_owned()).is_some();
                }
            }
            let expected = self.flow.lock().expect("flow lock").as_ref().filter(|f| f.phase == "waiting").map(|f| f.state.clone());
            let right = host_ok
                && !doubled
                && parsed.as_ref().is_some_and(|u| u.path() == "/auth/callback")
                && expected.is_some_and(|s| params.get("state").is_some_and(|got| constant_eq(got, &s)));
            let body = if right { "Finishing the sign-in. You can go back to Catbot." } else { "This isn't the sign-in Catbot is waiting for. Go back to Catbot and try again." };
            let status = if right { "200 OK" } else { "400 Bad Request" };
            let reply = format!(
                "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(reply.as_bytes()).await;
            let _ = socket.shutdown().await;
            if right {
                return Some(params);
            }
        }
    }

    fn finish(&self, flow_id: &str, phase: &'static str, message: Option<&str>) {
        if let Some(f) = self.flow.lock().expect("flow lock").as_mut().filter(|f| f.id == flow_id && f.phase == "waiting") {
            f.phase = phase;
            f.url = None;
            f.message = message.map(str::to_string);
        }
    }

    pub fn cancel(&self) {
        let id = self.flow.lock().expect("flow lock").as_ref().map(|f| f.id.clone());
        if let Some(id) = id {
            self.finish(&id, "cancelled", Some("The ChatGPT sign-in was cancelled."));
        }
    }

    async fn complete(&self, flow_id: &str, params: HashMap<String, String>) {
        let outcome = self.trade(flow_id, &params).await;
        match outcome {
            Ok(allowed) => self.finish(flow_id, if allowed { "succeeded" } else { "failed" }, (!allowed).then_some(CONSENT)),
            Err(problem) => self.finish(flow_id, "failed", Some(&problem)),
        }
    }

    /// The code for tokens; the identity checked; the account kept. Whether plan use is allowed.
    async fn trade(&self, flow_id: &str, params: &HashMap<String, String>) -> Result<bool, String> {
        let flow = self.flow.lock().expect("flow lock").clone().filter(|f| f.id == flow_id).ok_or("The sign-in is gone.")?;
        if params.contains_key("error") {
            return Err("The ChatGPT sign-in wasn't approved. You can try again.".into());
        }
        let client_id = params.get("client_id").cloned().or(flow.client_id.clone()).filter(|c| !c.is_empty() && c.len() <= 256 && c != "dynamic_agent_client");
        let code = params.get("code").filter(|c| !c.is_empty() && c.len() <= 4096);
        let (Some(client_id), Some(code)) = (client_id, code) else {
            return Err("ChatGPT's answer was incomplete. Start the sign-in again.".into());
        };
        if flow.client_id.as_ref().is_some_and(|c| *c != client_id) {
            return Err("ChatGPT answered for another registration. Start again.".into());
        }
        let keys = self.keys().await?;
        let (status, body) = self
            .post_form(&[
                ("grant_type", "authorization_code"),
                ("client_id", &client_id),
                ("code", code),
                ("code_verifier", &flow.verifier),
                ("redirect_uri", &flow.redirect_uri),
                ("resource", &self.resource),
            ])
            .await?;
        if !status.is_success() {
            if flow.previous.is_none() && body["error"] == "invalid_grant" {
                *self.retry_client.lock().expect("retry lock") = Some(client_id.clone());
            }
            return Err("ChatGPT couldn't finish this sign-in. Start again.".into());
        }
        let tokens = token_set(&body, None)?;
        let (subject, email) = self.identity(&tokens.id, &client_id, &keys, Some(&flow.nonce))?;
        if flow.previous.as_ref().is_some_and(|p| p.subject != subject) {
            return Err("ChatGPT signed in another account. The saved one was kept.".into());
        }
        if self.read()? != flow.previous {
            return Err("The ChatGPT sign-in changed while this one ran. Start again.".into());
        }
        let allowed = tokens.scopes.iter().any(|s| s == DIRECT_SCOPE);
        self.save(&Credentials { client_id, subject, email, host_id: flow.host_id, tokens: Some(tokens) })?;
        *self.retry_client.lock().expect("retry lock") = None;
        Ok(allowed)
    }

    async fn post_form(&self, form: &[(&str, &str)]) -> Result<(reqwest::StatusCode, Value), String> {
        let answer = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|e| e.to_string())?
            .post(format!("{}/api/accounts/oauth/token", self.issuer))
            .form(form)
            .send()
            .await
            .map_err(|_| "Couldn't reach ChatGPT. Check the connection and try again.".to_string())?;
        let status = answer.status();
        let bytes = answer.bytes().await.map_err(|e| e.to_string())?;
        if bytes.len() > 1_048_576 {
            return Err("ChatGPT's answer was too big.".into());
        }
        Ok((status, serde_json::from_slice(&bytes).unwrap_or(Value::Null)))
    }

    async fn keys(&self) -> Result<jsonwebtoken::jwk::JwkSet, String> {
        let answer = reqwest::Client::new()
            .get(format!("{}/.well-known/jwks.json", self.issuer))
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .map_err(|_| "ChatGPT's identity keys are out of reach. Try again later.".to_string())?;
        if !answer.status().is_success() {
            return Err("ChatGPT's identity keys are out of reach. Try again later.".into());
        }
        let set: jsonwebtoken::jwk::JwkSet = answer.json().await.map_err(|_| "ChatGPT's identity keys can't be read.".to_string())?;
        if set.keys.is_empty() {
            return Err("ChatGPT gave no identity keys.".into());
        }
        Ok(set)
    }

    /// Checks the identity token: signed by OpenAI, for this client, with the sign-in's nonce.
    fn identity(&self, token: &str, client_id: &str, keys: &jsonwebtoken::jwk::JwkSet, nonce: Option<&str>) -> Result<(String, Option<String>), String> {
        let bad = || "ChatGPT's identity couldn't be checked. Nothing was saved; start the sign-in again.".to_string();
        let header = jsonwebtoken::decode_header(token).map_err(|_| bad())?;
        if !matches!(header.alg, jsonwebtoken::Algorithm::RS256 | jsonwebtoken::Algorithm::ES256) {
            return Err(bad());
        }
        let jwk = match &header.kid {
            Some(kid) => keys.find(kid),
            None => keys.keys.first(),
        }
        .ok_or_else(bad)?;
        let key = jsonwebtoken::DecodingKey::from_jwk(jwk).map_err(|_| bad())?;
        let mut validation = jsonwebtoken::Validation::new(header.alg);
        validation.set_issuer(&[&self.issuer]);
        validation.set_audience(&[client_id]);
        validation.leeway = 30;
        validation.set_required_spec_claims(&["sub", "exp", "iat", "iss", "aud"]);
        let data = jsonwebtoken::decode::<Value>(token, &key, &validation).map_err(|_| bad())?;
        let claims = data.claims;
        if nonce.is_some_and(|n| claims["nonce"].as_str() != Some(n)) {
            return Err(bad());
        }
        if claims["iat"].as_i64().is_some_and(|iat| iat > chrono::Utc::now().timestamp() + 30) {
            return Err(bad());
        }
        let subject = claims["sub"].as_str().filter(|s| !s.is_empty() && s.len() <= 512).ok_or_else(bad)?.to_string();
        let email = claims["email"].as_str().filter(|e| e.len() <= 320).map(str::to_string);
        Ok((subject, email))
    }

    /// A token for a turn, renewed when it's near its end.
    pub async fn access_token(&self) -> Result<String, String> {
        let _one = self.renewing.lock().await;
        let mut record = self.read()?.ok_or(SIGN_IN)?;
        let tokens = record.tokens.clone().ok_or(SIGN_IN)?;
        if !tokens.scopes.iter().any(|s| s == DIRECT_SCOPE) {
            return Err(CONSENT.into());
        }
        if tokens.expires_at > now_ms() + 120_000 {
            return Ok(tokens.access);
        }
        if tokens.earliest_refresh_at.is_some_and(|at| at > now_ms()) {
            return if tokens.expires_at > now_ms() { Ok(tokens.access) } else { Err("ChatGPT's sign-in can't renew yet. Try again shortly.".into()) };
        }
        let keys = self.keys().await?;
        let (status, body) = self.post_form(&[("grant_type", "refresh_token"), ("client_id", &record.client_id), ("refresh_token", &tokens.refresh), ("resource", &self.resource)]).await?;
        if !status.is_success() {
            let code = body["error"].as_str().or(body["error"]["code"].as_str()).unwrap_or_default();
            if ["invalid_grant", "invalid_refresh_token", "token_expired", "refresh_token_expired", "refresh_token_invalidated", "refresh_token_reused"].contains(&code) {
                record.tokens = None;
                self.save(&record)?;
                return Err(SIGN_IN.into());
            }
            return Err("ChatGPT couldn't renew the sign-in right now. Try again later; it's still saved.".into());
        }
        // The old refresh token is spent: anything wrong with the new ones means signing in again.
        let renewed = token_set(&body, Some(&tokens)).and_then(|t| {
            if body["id_token"].is_string() {
                let (subject, _) = self.identity(&t.id, &record.client_id, &keys, None)?;
                if subject != record.subject {
                    return Err("ChatGPT renewed another account.".to_string());
                }
            }
            Ok(t)
        });
        match renewed {
            Ok(t) => {
                let access = t.access.clone();
                let allowed = t.scopes.iter().any(|s| s == DIRECT_SCOPE);
                record.tokens = Some(t);
                self.save(&record)?;
                if allowed { Ok(access) } else { Err(CONSENT.into()) }
            }
            Err(_) => {
                record.tokens = None;
                self.save(&record)?;
                Err("ChatGPT's renewal couldn't be checked. Continue with ChatGPT again.".into())
            }
        }
    }

    /// The models the plan offers: `[{ id, label }]`.
    pub async fn models(&self) -> Result<Vec<Value>, String> {
        let token = self.access_token().await?;
        let answer = reqwest::Client::new()
            .get(format!("{}/models", self.resource))
            .bearer_auth(token)
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .map_err(|_| "ChatGPT's models are out of reach right now.".to_string())?;
        if !answer.status().is_success() {
            return Err(format!("ChatGPT didn't list its models ({}).", answer.status()));
        }
        let body: Value = answer.json().await.map_err(|_| "ChatGPT's model list can't be read.".to_string())?;
        let mut out: Vec<Value> = Vec::new();
        for m in body["models"].as_array().into_iter().flatten() {
            let (Some(slug), Some(name)) = (m["slug"].as_str(), m["display_name"].as_str()) else { continue };
            if m["visibility"] != "list" || slug.is_empty() || slug.len() > 256 || slug.chars().any(|c| c.is_whitespace() || c.is_control()) {
                continue;
            }
            if !out.iter().any(|o| o["id"] == slug) {
                out.push(json!({ "id": slug, "label": name }));
            }
        }
        Ok(out)
    }

    /// Signs out here, and asks OpenAI to end the sign-in too. Whether OpenAI confirmed it.
    pub async fn sign_out(&self) -> Result<bool, String> {
        self.cancel();
        let _one = self.renewing.lock().await;
        let Some(mut record) = self.read()? else { return Ok(true) };
        let Some(tokens) = record.tokens.take() else { return Ok(true) };
        let mut revoked = tokens.refresh.is_empty();
        if !revoked {
            let client = reqwest::Client::new();
            let discovery: Option<Value> = async { client.get(format!("{}/.well-known/openid-configuration", self.issuer)).timeout(Duration::from_secs(15)).send().await.ok()?.json().await.ok() }.await;
            let endpoint = discovery
                .as_ref()
                .filter(|d| d["issuer"] == self.issuer.as_str())
                .and_then(|d| d["revocation_endpoint"].as_str())
                .filter(|e| e.starts_with(&format!("{}/", self.issuer)))
                .map(str::to_string);
            if let Some(endpoint) = endpoint {
                let form = [("token", tokens.refresh.as_str()), ("token_type_hint", "refresh_token"), ("client_id", record.client_id.as_str())];
                revoked = client.post(endpoint).form(&form).timeout(Duration::from_secs(15)).send().await.is_ok_and(|r| r.status().as_u16() == 200);
            }
        }
        self.save(&record)?;
        Ok(revoked)
    }
}

/// Tokens from a token answer (keeping the old identity token and scopes when a renewal leaves
/// them out).
fn token_set(body: &Value, previous: Option<&Tokens>) -> Result<Tokens, String> {
    let incomplete = || "ChatGPT gave incomplete credentials. Continue with ChatGPT again.".to_string();
    let access = body["access_token"].as_str().filter(|a| !a.is_empty()).ok_or_else(incomplete)?;
    if !body["token_type"].as_str().is_some_and(|t| t.eq_ignore_ascii_case("bearer")) {
        return Err(incomplete());
    }
    let expires_in = body["expires_in"].as_f64().filter(|e| *e > 0.0 && *e <= 86_400.0).ok_or_else(incomplete)?;
    let id = body["id_token"].as_str().map(str::to_string).or_else(|| previous.map(|p| p.id.clone())).ok_or_else(incomplete)?;
    let scopes: Vec<String> = match body["scope"].as_str() {
        Some(s) => s.split_whitespace().map(str::to_string).collect(),
        None => previous.map(|p| p.scopes.clone()).ok_or_else(incomplete)?,
    };
    let refresh = body["refresh_token"].as_str().unwrap_or_default().to_string();
    if scopes.iter().any(|s| s == DIRECT_SCOPE) && refresh.is_empty() {
        return Err("ChatGPT gave no way to renew the plan's sign-in. Continue with ChatGPT again.".into());
    }
    Ok(Tokens {
        access: access.to_string(),
        refresh,
        id,
        scopes,
        expires_at: now_ms() + (expires_in * 1000.0) as i64,
        earliest_refresh_at: body["earliest_refresh_at"].as_f64().map(|s| (s * 1000.0) as i64),
    })
}

/// Compares two secrets without stopping at the first difference.
fn constant_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// The Codex settings that point a turn at the plan: OpenAI's API with the token from
/// `CATBOT_CHATGPT_TOKEN` (which Codex keeps out of the bot's shell, as it does every *TOKEN*).
pub fn codex_args(resource: &str) -> Vec<String> {
    let quoted = serde_json::to_string(resource).unwrap_or_default();
    [
        "-c".to_string(),
        "model_provider=\"catbot_chatgpt_plan\"".to_string(),
        "-c".to_string(),
        "model_providers.catbot_chatgpt_plan.name=\"ChatGPT plan\"".to_string(),
        "-c".to_string(),
        format!("model_providers.catbot_chatgpt_plan.base_url={quoted}"),
        "-c".to_string(),
        "model_providers.catbot_chatgpt_plan.env_key=\"CATBOT_CHATGPT_TOKEN\"".to_string(),
        "-c".to_string(),
        "model_providers.catbot_chatgpt_plan.wire_api=\"responses\"".to_string(),
        "-c".to_string(),
        "model_providers.catbot_chatgpt_plan.requires_openai_auth=false".to_string(),
        "-c".to_string(),
        "model_providers.catbot_chatgpt_plan.supports_websockets=false".to_string(),
        "-c".to_string(),
        "cli_auth_credentials_store=\"ephemeral\"".to_string(),
        "-c".to_string(),
        "features.tool_search=false".to_string(),
    ]
    .to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_need_a_way_to_renew_plan_use() {
        let full = json!({ "access_token": "a", "token_type": "Bearer", "expires_in": 3600, "id_token": "i", "scope": format!("openid {DIRECT_SCOPE}"), "refresh_token": "r" });
        let t = token_set(&full, None).unwrap();
        assert!(t.scopes.iter().any(|s| s == DIRECT_SCOPE) && t.refresh == "r");
        let mut no_refresh = full.clone();
        no_refresh["refresh_token"] = Value::Null;
        assert!(token_set(&no_refresh, None).is_err());
        let renewal = json!({ "access_token": "b", "token_type": "bearer", "expires_in": 60, "refresh_token": "r2" });
        let kept = token_set(&renewal, Some(&t)).unwrap();
        assert_eq!((kept.id.as_str(), kept.scopes.len()), ("i", 2), "a renewal keeps the identity and scopes");
        assert!(token_set(&json!({ "access_token": "a", "token_type": "mac", "expires_in": 5 }), None).is_err());
        assert!(constant_eq("abc", "abc") && !constant_eq("abc", "abd") && !constant_eq("abc", "ab"));
    }

    #[test]
    fn codex_is_pointed_at_the_plan() {
        let args = codex_args("https://api.openai.com/v1").join(" ");
        assert!(args.contains("model_providers.catbot_chatgpt_plan.base_url=\"https://api.openai.com/v1\""));
        assert!(args.contains("env_key=\"CATBOT_CHATGPT_TOKEN\"") && args.contains("requires_openai_auth=false"));
    }
}
