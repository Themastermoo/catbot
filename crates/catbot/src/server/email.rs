//! Email (stage 13.9): the person's mailbox, for the bots they choose.
//!
//! Moo's choice (2026-11-16): a mailbox connected in Settings now (IMAP and SMTP, any provider;
//! Gmail and the like with an app password), and Gmail and Outlook as connected apps in group 2.
//! Bots read and search freely; sending always asks the person first. The password stays in
//! Catbot's database and never goes to a bot or back to the app.
//!
//! The mailbox is reached over TLS (or STARTTLS), never in the clear, except on this computer
//! itself (a mail bridge such as Proton Mail Bridge listens on 127.0.0.1).

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::http::{Method, StatusCode};
use futures_util::TryStreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;

use mail_parser::MimeHeaders;

use super::bot_tools::Grant;
use super::{Answer, App, bad, fail};

const SETTING: &str = "email";
const CONNECT_TIME: std::time::Duration = std::time::Duration::from_secs(20);
/// The most of a message's text a bot gets at once.
const MOST_TEXT: usize = 20_000;

/// How a mail server speaks: TLS from the first byte, upgraded with STARTTLS, or (on this
/// computer only) plain.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Security {
    #[default]
    Tls,
    Starttls,
    Plain,
}

/// The mailbox, kept in the settings as `email`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Mailbox {
    pub address: String,
    /// The sign-in name, when it isn't the address.
    pub username: String,
    pub password: String,
    pub imap_host: String,
    pub imap_port: u16,
    pub imap_security: Security,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_security: Security,
    /// The bots that may use it (none until the person picks).
    pub bots: Vec<String>,
}

impl Mailbox {
    fn login(&self) -> &str {
        if self.username.trim().is_empty() { self.address.trim() } else { self.username.trim() }
    }
}

/// The servers for a well-known address, filled in for the person.
pub fn guess(address: &str) -> Option<(&'static str, u16, Security, &'static str, u16, Security)> {
    let domain = address.rsplit('@').next()?.to_lowercase();
    Some(match domain.as_str() {
        "gmail.com" | "googlemail.com" => ("imap.gmail.com", 993, Security::Tls, "smtp.gmail.com", 465, Security::Tls),
        "outlook.com" | "hotmail.com" | "live.com" | "msn.com" => ("outlook.office365.com", 993, Security::Tls, "smtp.office365.com", 587, Security::Starttls),
        "yahoo.com" | "ymail.com" => ("imap.mail.yahoo.com", 993, Security::Tls, "smtp.mail.yahoo.com", 465, Security::Tls),
        "icloud.com" | "me.com" | "mac.com" => ("imap.mail.me.com", 993, Security::Tls, "smtp.mail.me.com", 587, Security::Starttls),
        "aol.com" => ("imap.aol.com", 993, Security::Tls, "smtp.aol.com", 465, Security::Tls),
        "fastmail.com" | "fastmail.fm" => ("imap.fastmail.com", 993, Security::Tls, "smtp.fastmail.com", 465, Security::Tls),
        "zoho.com" => ("imap.zoho.com", 993, Security::Tls, "smtp.zoho.com", 465, Security::Tls),
        "gmx.com" | "gmx.net" => ("imap.gmx.com", 993, Security::Tls, "mail.gmx.com", 465, Security::Tls),
        _ => return None,
    })
}

fn is_loopback(host: &str) -> bool {
    matches!(host.trim(), "localhost" | "127.0.0.1" | "::1" | "[::1]")
}

pub fn mailbox(app: &App) -> Option<Mailbox> {
    app.with_store(|store| store.setting(SETTING)).flatten().and_then(|v| serde_json::from_value(v).ok()).filter(|m: &Mailbox| !m.address.is_empty())
}

/// The connection to a mail server, plain or TLS, as one stream.
#[derive(Debug)]
pub enum Wire {
    Plain(TcpStream),
    Tls(Box<tokio_rustls::client::TlsStream<TcpStream>>),
}

impl AsyncRead for Wire {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Wire::Plain(s) => Pin::new(s).poll_read(cx, buf),
            Wire::Tls(s) => Pin::new(s.as_mut()).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Wire {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
        match self.get_mut() {
            Wire::Plain(s) => Pin::new(s).poll_write(cx, buf),
            Wire::Tls(s) => Pin::new(s.as_mut()).poll_write(cx, buf),
        }
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Wire::Plain(s) => Pin::new(s).poll_flush(cx),
            Wire::Tls(s) => Pin::new(s.as_mut()).poll_flush(cx),
        }
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Wire::Plain(s) => Pin::new(s).poll_shutdown(cx),
            Wire::Tls(s) => Pin::new(s.as_mut()).poll_shutdown(cx),
        }
    }
}

/// Wraps a connection in TLS for `host`, checked against the usual certificate authorities.
pub async fn tls(stream: TcpStream, host: &str) -> Result<Wire, String> {
    let mut roots = tokio_rustls::rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let config = tokio_rustls::rustls::ClientConfig::builder_with_provider(Arc::new(tokio_rustls::rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_root_certificates(roots)
        .with_no_client_auth();
    let name = tokio_rustls::rustls::pki_types::ServerName::try_from(host.to_string()).map_err(|_| format!("{host} isn't a server name"))?;
    let tls = tokio_rustls::TlsConnector::from(Arc::new(config)).connect(name, stream).await.map_err(|e| format!("a secure connection to {host} failed: {e}"))?;
    Ok(Wire::Tls(Box::new(tls)))
}

type Session = async_imap::Session<Wire>;

fn imap_error(e: impl std::fmt::Display) -> String {
    let said = e.to_string();
    if said.contains("AUTHENTICATIONFAILED") || said.to_lowercase().contains("invalid credentials") || said.to_lowercase().contains("login failed") {
        "the mail server didn't accept the address and password (Gmail, iCloud and Yahoo need an app password)".into()
    } else {
        format!("the mail server said: {said}")
    }
}

/// Signs in to the mailbox's IMAP server.
async fn open(m: &Mailbox) -> Result<Session, String> {
    let host = m.imap_host.trim();
    if m.imap_security == Security::Plain && !is_loopback(host) {
        return Err("Catbot only reaches a mail server without TLS on this computer itself".into());
    }
    let work = async {
        let stream = TcpStream::connect((host, m.imap_port)).await.map_err(|e| format!("couldn't reach {host}:{}: {e}", m.imap_port))?;
        let wire = match m.imap_security {
            Security::Tls => tls(stream, host).await?,
            Security::Plain => Wire::Plain(stream),
            Security::Starttls => {
                let mut client = async_imap::Client::new(Wire::Plain(stream));
                client.read_response().await.map_err(imap_error)?;
                client.run_command_and_check_ok("STARTTLS", None).await.map_err(imap_error)?;
                let Wire::Plain(stream) = client.into_inner() else { unreachable!("plain until now") };
                tls(stream, host).await?
            }
        };
        let mut client = async_imap::Client::new(wire);
        if m.imap_security != Security::Starttls {
            client.read_response().await.map_err(imap_error)?;
        }
        client.login(m.login(), &m.password).await.map_err(|(e, _)| imap_error(e))
    };
    tokio::time::timeout(CONNECT_TIME, work).await.map_err(|_| format!("{host} took too long to answer"))?
}

/// Turns the bot's search words into an IMAP search: words, and `from:`, `to:`, `subject:`,
/// `since:`/`before:` (YYYY-MM-DD) and `unread`.
pub fn criteria(query: &str) -> Result<String, String> {
    let mut parts: Vec<String> = Vec::new();
    let quote = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
    let date = |s: &str| {
        chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").map(|d| d.format("%-d-%b-%Y").to_string()).map_err(|_| format!("{s} isn't a date like 2026-11-16"))
    };
    // Words, keeping "quoted phrases" whole.
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for c in query.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !current.is_empty() {
                    words.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    for word in words {
        let (key, value) = word.split_once(':').filter(|(k, v)| !v.is_empty() && ["from", "to", "subject", "since", "before"].contains(&k.to_lowercase().as_str())).unwrap_or(("", &word));
        parts.push(match key.to_lowercase().as_str() {
            "from" => format!("FROM {}", quote(value)),
            "to" => format!("TO {}", quote(value)),
            "subject" => format!("SUBJECT {}", quote(value)),
            "since" => format!("SINCE {}", date(value)?),
            "before" => format!("BEFORE {}", date(value)?),
            _ if word.eq_ignore_ascii_case("unread") => "UNSEEN".to_string(),
            _ => format!("TEXT {}", quote(&word)),
        });
    }
    Ok(if parts.is_empty() { "ALL".into() } else { parts.join(" ") })
}

fn folder_of(args: &Value) -> String {
    args["folder"].as_str().map(str::trim).filter(|f| !f.is_empty()).unwrap_or("INBOX").to_string()
}

/// A message's headers in one line for a list.
fn line(uid: u32, header: &[u8], unread: bool) -> String {
    let parsed = mail_parser::MessageParser::default().parse(header);
    let (from, subject, date) = match &parsed {
        Some(m) => (
            m.from().and_then(|a| a.first()).map(|a| match (a.name(), a.address()) {
                (Some(name), Some(address)) => format!("{name} <{address}>"),
                (_, Some(address)) => address.to_string(),
                (Some(name), None) => name.to_string(),
                _ => String::new(),
            }),
            m.subject().map(str::to_string),
            m.date().map(|d| d.to_rfc3339()),
        ),
        None => (None, None, None),
    };
    format!(
        "id {uid} · {} · from {} · “{}”{}",
        date.unwrap_or_else(|| "no date".into()),
        from.unwrap_or_else(|| "unknown".into()),
        subject.unwrap_or_else(|| "(no subject)".into()),
        if unread { " · unread" } else { "" }
    )
}

async fn search(m: &Mailbox, args: &Value) -> Result<String, String> {
    let folder = folder_of(args);
    let query = criteria(args["query"].as_str().unwrap_or_default())?;
    let limit = args["limit"].as_u64().unwrap_or(20).clamp(1, 50) as usize;
    let mut session = open(m).await?;
    session.examine(&folder).await.map_err(|e| format!("couldn't open the folder {folder}: {}", imap_error(e)))?;
    let mut uids: Vec<u32> = session.uid_search(&query).await.map_err(imap_error)?.into_iter().collect();
    uids.sort_unstable_by_key(|uid| std::cmp::Reverse(*uid));
    let total = uids.len();
    uids.truncate(limit);
    let mut lines = Vec::new();
    if !uids.is_empty() {
        let set = uids.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
        let fetched: Vec<_> = session.uid_fetch(&set, "(UID FLAGS BODY.PEEK[HEADER])").await.map_err(imap_error)?.try_collect().await.map_err(imap_error)?;
        let mut rows: Vec<(u32, String)> = fetched
            .iter()
            .filter_map(|f| {
                let uid = f.uid?;
                let unread = !f.flags().any(|flag| matches!(flag, async_imap::types::Flag::Seen));
                Some((uid, line(uid, f.header().unwrap_or_default(), unread)))
            })
            .collect();
        rows.sort_unstable_by_key(|row| std::cmp::Reverse(row.0));
        lines = rows.into_iter().map(|(_, l)| l).collect();
    }
    let _ = session.logout().await;
    if lines.is_empty() {
        return Ok(format!("Nothing in {folder} matches."));
    }
    let more = if total > lines.len() { format!("\n({total} match in all; these are the newest {}.)", lines.len()) } else { String::new() };
    Ok(format!("In {folder}, newest first:\n{}{more}", lines.join("\n")))
}

async fn read(m: &Mailbox, args: &Value) -> Result<String, String> {
    let folder = folder_of(args);
    let uid = args["id"].as_u64().or_else(|| args["id"].as_str().and_then(|s| s.trim().parse().ok())).ok_or("which message? give its id from mail_search")?;
    let mut session = open(m).await?;
    session.examine(&folder).await.map_err(|e| format!("couldn't open the folder {folder}: {}", imap_error(e)))?;
    let fetched: Vec<_> = session.uid_fetch(uid.to_string(), "(UID BODY.PEEK[])").await.map_err(imap_error)?.try_collect().await.map_err(imap_error)?;
    let _ = session.logout().await;
    let raw = fetched.iter().find_map(|f| f.body()).ok_or_else(|| format!("there's no message {uid} in {folder}"))?;
    Ok(render(uid as u32, raw))
}

/// A whole message, as a bot reads it.
pub fn render(uid: u32, raw: &[u8]) -> String {
    let Some(m) = mail_parser::MessageParser::default().parse(raw) else { return format!("Message {uid} couldn't be read.") };
    let people = |list: Option<&mail_parser::Address>| {
        list.map(|a| {
            a.iter()
                .map(|p| match (p.name(), p.address()) {
                    (Some(n), Some(a)) => format!("{n} <{a}>"),
                    (_, Some(a)) => a.to_string(),
                    (Some(n), None) => n.to_string(),
                    _ => String::new(),
                })
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default()
    };
    let mut text = format!("Message {uid}\nFrom: {}\nTo: {}\n", people(m.from()), people(m.to()));
    let cc = people(m.cc());
    if !cc.is_empty() {
        text.push_str(&format!("Cc: {cc}\n"));
    }
    text.push_str(&format!(
        "Date: {}\nSubject: {}\n",
        m.date().map(|d| d.to_rfc3339()).unwrap_or_default(),
        m.subject().unwrap_or("(no subject)")
    ));
    let names: Vec<String> = m.attachments().filter_map(|a| a.attachment_name().map(str::to_string)).collect();
    if !names.is_empty() {
        text.push_str(&format!("Attachments: {}\n", names.join(", ")));
    }
    let body = m.body_text(0).map(|b| b.to_string()).unwrap_or_default();
    let body: String = body.chars().take(MOST_TEXT).collect();
    text.push('\n');
    text.push_str(body.trim());
    if body.chars().count() >= MOST_TEXT {
        text.push_str("\n(…cut short)");
    }
    text
}

async fn folders(m: &Mailbox) -> Result<String, String> {
    let mut session = open(m).await?;
    let names: Vec<String> = session.list(Some(""), Some("*")).await.map_err(imap_error)?.map_ok(|n| n.name().to_string()).try_collect().await.map_err(imap_error)?;
    let _ = session.logout().await;
    Ok(format!("Folders: {}", names.join(", ")))
}

/// The mail tools a bot has: none unless a mailbox is connected and it's one of the chosen bots.
pub fn tools_for(app: &App, bot_id: &str) -> Vec<Value> {
    let Some(m) = mailbox(app) else { return Vec::new() };
    if !m.bots.iter().any(|b| b == bot_id) {
        return Vec::new();
    }
    let folder = json!({ "type": "string", "description": "A folder; INBOX if left out (mail_folders lists them)." });
    vec![
        json!({ "name": "mail_search", "description": format!("Search the person's mailbox ({}), newest first. query takes words and from:, to:, subject:, since:YYYY-MM-DD, before:YYYY-MM-DD and unread; leave it out for the newest mail. Each result has an id for mail_read. Reading doesn't mark anything read.", m.address),
            "inputSchema": { "type": "object", "properties": { "query": { "type": "string" }, "folder": folder, "limit": { "type": "integer", "description": "At most this many (20 if left out, 50 at most)." } } } }),
        json!({ "name": "mail_read", "description": "Read one message from the person's mailbox: who it's from and to, the subject, the text and the attachments' names.",
            "inputSchema": { "type": "object", "properties": { "id": { "type": "integer", "description": "The id from mail_search." }, "folder": folder }, "required": ["id"] } }),
        json!({ "name": "mail_folders", "description": "List the folders in the person's mailbox.", "inputSchema": { "type": "object", "properties": {} } }),
    ]
}

/// A bot's call to one of the mail tools.
pub async fn call(app: &Arc<App>, grant: &Grant, name: &str, args: &Value) -> Result<String, String> {
    let m = mailbox(app).ok_or("no mailbox is connected any more")?;
    if !m.bots.contains(&grant.bot_id) {
        return Err("this bot may no longer use the mailbox".into());
    }
    match name {
        "mail_search" => search(&m, args).await,
        "mail_read" => read(&m, args).await,
        "mail_folders" => folders(&m).await,
        other => Err(format!("unknown mail tool {other}")),
    }
}

/// The mailbox as the app sees it: never the password.
fn view(app: &App) -> Value {
    match mailbox(app) {
        Some(m) => json!({
            "connected": true,
            "address": m.address,
            "username": m.username,
            "imapHost": m.imap_host, "imapPort": m.imap_port, "imapSecurity": m.imap_security,
            "smtpHost": m.smtp_host, "smtpPort": m.smtp_port, "smtpSecurity": m.smtp_security,
            "bots": m.bots,
        }),
        None => json!({ "connected": false }),
    }
}

/// `GET /api/email`; at home also `PUT /api/email` (connect or change: tries the mailbox
/// first), `PUT /api/email/bots` `{ bots }`, `DELETE /api/email`, and
/// `GET /api/email/guess?address=` (the servers for a well-known address).
pub async fn route(app: &Arc<App>, method: &Method, rest: &[&str], body: Option<Value>, query: Option<String>) -> Answer {
    let body = body.unwrap_or_default();
    match (method, rest) {
        (&Method::GET, []) => Ok((StatusCode::OK, view(app))),
        (&Method::GET, ["guess"]) => {
            let address = query.unwrap_or_default();
            Ok((StatusCode::OK, match guess(&address) {
                Some((ih, ip, is, sh, sp, ss)) => json!({ "known": true, "imapHost": ih, "imapPort": ip, "imapSecurity": is, "smtpHost": sh, "smtpPort": sp, "smtpSecurity": ss }),
                None => json!({ "known": false }),
            }))
        }
        (&Method::PUT, []) => {
            let before = mailbox(app).unwrap_or_default();
            let mut m: Mailbox = serde_json::from_value(body.clone()).map_err(|e| bad(format!("the mailbox: {e}")))?;
            m.address = m.address.trim().to_string();
            if !m.address.contains('@') {
                return Err(bad("an email address, please"));
            }
            if m.password.is_empty() && before.address == m.address {
                m.password = before.password.clone();
            }
            if m.password.is_empty() {
                return Err(bad("the mailbox's password (or app password)"));
            }
            if let Some((ih, ip, is, sh, sp, ss)) = guess(&m.address) {
                if m.imap_host.trim().is_empty() {
                    (m.imap_host, m.imap_port, m.imap_security) = (ih.into(), ip, is);
                }
                if m.smtp_host.trim().is_empty() {
                    (m.smtp_host, m.smtp_port, m.smtp_security) = (sh.into(), sp, ss);
                }
            }
            if m.imap_host.trim().is_empty() {
                return Err(bad("Catbot doesn't know this address's mail servers: fill them in"));
            }
            if m.imap_port == 0 {
                m.imap_port = if m.imap_security == Security::Tls { 993 } else { 143 };
            }
            if m.smtp_port == 0 {
                m.smtp_port = if m.smtp_security == Security::Tls { 465 } else { 587 };
            }
            if body.get("bots").is_none() {
                m.bots = before.bots.clone();
            }
            // It's kept only once the mailbox lets Catbot in.
            let mut session = open(&m).await.map_err(|why| (StatusCode::BAD_REQUEST, why))?;
            let _ = session.logout().await;
            app.try_store(|store| store.set_setting(SETTING, &json!(m))).map_err(fail)?;
            changed(app, &before.bots, &m.bots);
            Ok((StatusCode::OK, view(app)))
        }
        (&Method::PUT, ["bots"]) => {
            let mut m = mailbox(app).ok_or_else(|| bad("connect a mailbox first"))?;
            let before = m.bots.clone();
            m.bots = body["bots"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
            app.try_store(|store| store.set_setting(SETTING, &json!(m))).map_err(fail)?;
            changed(app, &before, &m.bots);
            Ok((StatusCode::OK, view(app)))
        }
        (&Method::DELETE, []) => {
            let before = mailbox(app).map(|m| m.bots).unwrap_or_default();
            app.try_store(|store| store.set_setting(SETTING, &json!({}))).map_err(fail)?;
            changed(app, &before, &[]);
            Ok((StatusCode::OK, view(app)))
        }
        _ => Err((StatusCode::NOT_FOUND, "no such email route".into())),
    }
}

fn changed(app: &App, before: &[String], after: &[String]) {
    app.hub.send(json!({ "kind": "email" }));
    for id in before.iter().chain(after) {
        app.announce_bot(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_words_become_imap_criteria() {
        assert_eq!(criteria("").unwrap(), "ALL");
        assert_eq!(criteria("from:bob@x.com unread invoice").unwrap(), "FROM \"bob@x.com\" UNSEEN TEXT \"invoice\"");
        assert_eq!(criteria("subject:\"garden plan\" since:2026-11-02").unwrap(), "SUBJECT \"garden plan\" SINCE 2-Nov-2026");
        assert!(criteria("before:yesterday").is_err());
    }

    #[test]
    fn well_known_addresses_fill_in_their_servers() {
        assert_eq!(guess("moo@gmail.com").map(|g| g.0), Some("imap.gmail.com"));
        assert!(guess("moo@example.org").is_none());
        assert!(is_loopback("127.0.0.1") && !is_loopback("mail.example.org"));
    }

    #[test]
    fn a_message_reads_as_text() {
        let raw = b"From: Bob <bob@x.com>\r\nTo: Moo <moo@y.com>\r\nSubject: Seeds\r\nDate: Mon, 16 Nov 2026 09:00:00 -0600\r\n\r\nThe seeds came.\r\n";
        let said = render(7, raw);
        assert!(said.starts_with("Message 7\nFrom: Bob <bob@x.com>\nTo: Moo <moo@y.com>"), "{said}");
        assert!(said.contains("Subject: Seeds") && said.ends_with("The seeds came."), "{said}");
        assert!(line(7, raw, true).contains("from Bob <bob@x.com> · “Seeds” · unread"));
    }
}
