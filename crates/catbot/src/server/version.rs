//! This Catbot's version, and whether a newer one is out (stage 19.7.4).
//!
//! Moo's choice (2026-11-24): Catbot moves to GitHub soon, and updates are checked there. Until
//! a repository is named (`owner/name`, in Settings › This app), there's nothing to check. With
//! one, the check reads its latest release (a `v1.2.3` tag beside this version); a repository
//! with no releases is read by its newest commit, beside when this Catbot was built. Only the
//! public GitHub API is asked, with nothing of the person's.
//!
//! * `GET  /api/version`        `{ version, built, repo, checked }`
//! * `PUT  /api/version`        `{ repo }` ("" to forget it)
//! * `POST /api/version/check`  `{ latest: { kind, name, url, at }, newer }`

use std::time::Duration;

use hyper::{Method, StatusCode};
use serde_json::{Value, json};

use super::{Answer, App, bad, fail};

const KEY: &str = "updates";

/// Catbot's GitHub repository, `owner/name`, where every Catbot looks for updates unless
/// Settings › This app names another.
///
/// PLACEHOLDER: Moo makes the repository later (2026-11-25). When he gives its name, put it
/// here, e.g. `Some("moo/catbot")`, and see scripts/release/README.md for the rest.
pub const REPO: Option<&str> = None;

/// The version this server was built as.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// When this program was built: its file's time.
fn built() -> Option<chrono::DateTime<chrono::Utc>> {
    let exe = std::env::current_exe().ok()?;
    let modified = std::fs::metadata(exe).ok()?.modified().ok()?;
    Some(modified.into())
}

fn saved(app: &App) -> Value {
    app.with_store(|store| store.setting(KEY)).flatten().unwrap_or_else(|| json!({}))
}

/// Whether `owner/name` could be a GitHub repository.
pub fn check_repo(repo: &str) -> Result<String, String> {
    let repo = repo.trim().trim_start_matches("https://github.com/").trim_end_matches('/').trim_end_matches(".git").to_string();
    let parts: Vec<&str> = repo.split('/').collect();
    let fits = |p: &str| !p.is_empty() && !p.starts_with('.') && p.len() <= 100 && p.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if parts.len() == 2 && parts.iter().all(|p| fits(p)) {
        Ok(repo)
    } else {
        Err("name the repository as owner/name, like moo/catbot".into())
    }
}

/// `1.2.3` (or `v1.2.3`) as numbers, to compare.
pub fn numbers(version: &str) -> Vec<u64> {
    version.trim().trim_start_matches(['v', 'V']).split(['.', '-', '+']).take(3).map(|n| n.parse().unwrap_or(0)).collect()
}

pub fn newer(theirs: &str, ours: &str) -> bool {
    let (mut a, mut b) = (numbers(theirs), numbers(ours));
    a.resize(3, 0);
    b.resize(3, 0);
    a > b
}

async fn github(app: &App, path: &str) -> Result<(u16, Value), String> {
    let api = app.updates.place().api.clone();
    let client = reqwest::Client::builder().timeout(Duration::from_secs(10)).user_agent(format!("Catbot/{VERSION}")).build().map_err(|e| e.to_string())?;
    let reply = client.get(format!("{api}/repos/{path}")).header("Accept", "application/vnd.github+json").send().await.map_err(|e| format!("couldn't reach GitHub: {e}"))?;
    let status = reply.status().as_u16();
    Ok((status, reply.json().await.unwrap_or(Value::Null)))
}

/// The repository updates come from: the one named in Settings, or Catbot's own.
pub fn repo(app: &App) -> Option<String> {
    saved(app)["repo"].as_str().filter(|r| !r.is_empty()).map(str::to_string).or(REPO.map(str::to_string))
}

async fn check(app: &App) -> Answer {
    let repo = repo(app).unwrap_or_default();
    if repo.is_empty() {
        return Err(bad("name Catbot's GitHub repository first"));
    }
    let (status, release) = github(app, &format!("{repo}/releases/latest")).await.map_err(|e| (StatusCode::BAD_GATEWAY, e))?;
    let mut offer = Value::Null;
    let (latest, is_newer) = match status {
        200 => {
            let tag = release["tag_name"].as_str().unwrap_or_default().to_string();
            let is_newer = newer(&tag, VERSION);
            // What this device could install (stage 28.2); a release without release.json is
            // still worth telling about.
            if is_newer {
                let found = super::updates::release_json(&release).await.ok().flatten();
                offer = super::updates::offer(app, &release, found.as_ref());
            }
            (json!({ "kind": "release", "name": tag, "url": release["html_url"], "at": release["published_at"] }), is_newer)
        }
        404 => {
            let (status, commits) = github(app, &format!("{repo}/commits?per_page=1")).await.map_err(|e| (StatusCode::BAD_GATEWAY, e))?;
            if status == 404 {
                return Err((StatusCode::NOT_FOUND, format!("GitHub has no public repository {repo} (a private one can't be checked yet)")));
            }
            if status != 200 {
                return Err((StatusCode::BAD_GATEWAY, format!("GitHub answered {status}")));
            }
            let commit = &commits[0];
            let at = commit["commit"]["committer"]["date"].as_str().unwrap_or_default().to_string();
            let when = chrono::DateTime::parse_from_rfc3339(&at).ok().map(|t| t.with_timezone(&chrono::Utc));
            let sha: String = commit["sha"].as_str().unwrap_or_default().chars().take(7).collect();
            let later = matches!((when, built()), (Some(theirs), Some(ours)) if theirs > ours + chrono::Duration::minutes(5));
            (json!({ "kind": "commit", "name": sha, "url": commit["html_url"], "at": at, "message": commit["commit"]["message"].as_str().and_then(|m| m.lines().next()) }), later)
        }
        403 | 429 => return Err((StatusCode::BAD_GATEWAY, "GitHub says to wait: too many checks for now".into())),
        other => return Err((StatusCode::BAD_GATEWAY, format!("GitHub answered {other}"))),
    };
    let checked = json!({ "at": chrono::Utc::now().to_rfc3339(), "latest": latest, "newer": is_newer, "offer": offer });
    let mut kept = saved(app);
    kept["checked"] = checked.clone();
    app.try_store(|store| store.set_setting(KEY, &kept)).map_err(fail)?;
    Ok((StatusCode::OK, checked))
}

/// The repository's latest release, as GitHub gives it.
pub async fn latest_release(app: &App, repo: &str) -> Result<Value, String> {
    match github(app, &format!("{repo}/releases/latest")).await? {
        (200, release) => Ok(release),
        (404, _) => Err(format!("{repo} has no releases on GitHub yet")),
        (403 | 429, _) => Err("GitHub says to wait: too many checks for now".into()),
        (other, _) => Err(format!("GitHub answered {other}")),
    }
}

fn wire(app: &App) -> Value {
    let kept = saved(app);
    json!({
        "version": VERSION,
        "built": built().map(|t| t.to_rfc3339()),
        "repo": repo(app),
        "own_repo": REPO,
        "checked": kept["checked"],
        "update": super::updates::wire(app),
    })
}

pub async fn route(app: &std::sync::Arc<App>, method: &Method, rest: &[&str], body: Value) -> Answer {
    if method == Method::POST
        && let Some(answer) = super::updates::route(app, rest, body.clone()).await
    {
        return answer;
    }
    match (method, rest) {
        (&Method::GET, []) => Ok((StatusCode::OK, wire(app))),
        (&Method::PUT, []) => {
            let given = body["repo"].as_str().unwrap_or_default();
            let mut kept = saved(app);
            if given.trim().is_empty() {
                kept = json!({});
            } else {
                let repo = check_repo(given).map_err(bad)?;
                if kept["repo"] != json!(repo) {
                    kept = json!({ "repo": repo });
                }
            }
            app.try_store(|store| store.set_setting(KEY, &kept)).map_err(fail)?;
            Ok((StatusCode::OK, wire(app)))
        }
        (&Method::POST, ["check"]) => check(app).await,
        _ => Err((StatusCode::NOT_FOUND, "no such route".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_by_their_numbers() {
        assert!(newer("v0.2.0", "0.1.9"));
        assert!(newer("1.0", "0.9.9"));
        assert!(!newer("v0.1.0", "0.1.0"));
        assert!(!newer("0.1.0-beta", "0.1.0"));
        assert!(newer("0.1.10", "0.1.9"), "by number, not by letter");
    }

    #[test]
    fn repositories_are_owner_and_name() {
        assert_eq!(check_repo(" moo/catbot ").as_deref(), Ok("moo/catbot"));
        assert_eq!(check_repo("https://github.com/moo/catbot.git").as_deref(), Ok("moo/catbot"));
        assert!(check_repo("moo").is_err());
        assert!(check_repo("moo/cat bot").is_err());
        assert!(check_repo("../../x/y/z").is_err());
        assert!(check_repo("moo/..").is_err());
    }
}
