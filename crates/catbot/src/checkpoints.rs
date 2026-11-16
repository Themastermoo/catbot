//! Checkpoints: a snapshot of a bot's working folder before and after each turn, so the person
//! can see what the bot changed and put the folder back.
//!
//! Each bot gets a private git repository under Catbot's home (`checkpoints/<bot id>.git`)
//! whose work tree is the bot's folder. Every git call names that repository and that folder
//! (`GIT_DIR`, `GIT_WORK_TREE`) and ignores the person's own git settings, so a repository the
//! person keeps in the folder is never read or touched (its `.git` is left out). Following
//! MausBot's `server/checkpoints.ts` (itself after Cline, Roo-Code and Gemini CLI).
//!
//! Best effort: without git, or when git fails, a turn just has no checkpoint.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

/// Never snapshotted: other repositories, dependencies and build output (big, and rebuilt),
/// and the person's attachments (theirs, not the bot's work).
const EXCLUDES: &str = "\
.git/
.svn/
.hg/
attachments/
node_modules/
.venv/
venv/
__pycache__/
target/
dist/
build/
.next/
.gradle/
*.log
";

/// The most of a patch the interface is sent.
const MOST_PATCH: usize = 400 * 1024;

/// One bot's checkpoints.
#[derive(Debug, Clone)]
pub struct Checkpoints {
    repo: PathBuf,
    folder: PathBuf,
}

/// A file a turn changed.
#[derive(Debug, Clone, PartialEq)]
pub struct Changed {
    pub path: String,
    /// "added", "changed", "deleted".
    pub how: &'static str,
    /// Lines added and removed (None for binary files).
    pub added: Option<u64>,
    pub removed: Option<u64>,
}

impl Checkpoints {
    /// The checkpoints of the bot whose folder is `folder`, kept under `home`.
    pub fn new(home: &Path, bot_id: &str, folder: &Path) -> Checkpoints {
        Checkpoints { repo: home.join("checkpoints").join(format!("{bot_id}.git")), folder: folder.to_path_buf() }
    }

    fn git(&self, args: &[&str]) -> Result<String, String> {
        let mut command = Command::new("git");
        command
            .args(["-c", "core.autocrlf=false", "-c", "core.quotepath=false", "-c", "commit.gpgsign=false", "-c", "user.name=Catbot", "-c", "user.email=catbot@localhost"])
            .args(args)
            .env("GIT_DIR", &self.repo)
            .env("GIT_WORK_TREE", &self.folder)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", if cfg!(windows) { "NUL" } else { "/dev/null" })
            .env("GIT_TERMINAL_PROMPT", "0")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_OBJECT_DIRECTORY")
            .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
            .current_dir(&self.folder);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let out = command.output().map_err(|e| format!("git isn't available ({e})"))?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    fn ready(&self) -> Result<(), String> {
        if !self.folder.is_dir() {
            return Err("the bot's folder is missing".into());
        }
        if !self.repo.join("HEAD").exists() {
            std::fs::create_dir_all(&self.repo).map_err(|e| e.to_string())?;
            self.git(&["init", "-q"])?;
        }
        // Rewritten each time, so a newer exclude list reaches older repositories.
        let info = self.repo.join("info");
        std::fs::create_dir_all(&info).map_err(|e| e.to_string())?;
        std::fs::write(info.join("exclude"), EXCLUDES).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Snapshots the folder as it is now; the snapshot's id.
    pub fn snapshot(&self, label: &str) -> Result<String, String> {
        self.ready()?;
        self.git(&["add", "-A", "."])?;
        self.git(&["commit", "-q", "--allow-empty", "--no-verify", "-m", label])?;
        Ok(self.git(&["rev-parse", "HEAD"])?.trim().to_string())
    }

    /// What changed between two snapshots.
    pub fn changes(&self, before: &str, after: &str) -> Result<Vec<Changed>, String> {
        check(before)?;
        check(after)?;
        let mut files: Vec<Changed> = Vec::new();
        for line in self.git(&["diff", "--name-status", "--no-renames", before, after])?.lines() {
            let Some((status, path)) = line.split_once('\t') else { continue };
            let how = match status.chars().next() {
                Some('A') => "added",
                Some('D') => "deleted",
                _ => "changed",
            };
            files.push(Changed { path: path.to_string(), how, added: None, removed: None });
        }
        for line in self.git(&["diff", "--numstat", "--no-renames", before, after])?.lines() {
            let mut parts = line.splitn(3, '\t');
            let (Some(added), Some(removed), Some(path)) = (parts.next(), parts.next(), parts.next()) else { continue };
            if let Some(file) = files.iter_mut().find(|f| f.path == path) {
                file.added = added.parse().ok();
                file.removed = removed.parse().ok();
            }
        }
        Ok(files)
    }

    /// The changes between two snapshots as a patch (cut short if it's very long).
    pub fn patch(&self, before: &str, after: &str) -> Result<String, String> {
        check(before)?;
        check(after)?;
        let mut patch = self.git(&["diff", "--no-renames", "--no-color", before, after])?;
        if patch.len() > MOST_PATCH {
            let mut cut = MOST_PATCH;
            while !patch.is_char_boundary(cut) {
                cut -= 1;
            }
            patch.truncate(cut);
            patch.push_str("\n… (the rest is too long to show)\n");
        }
        Ok(patch)
    }

    /// Puts the folder back as it was at `snapshot`. What's there now is snapshotted first (its
    /// id is returned), so this can itself be undone. Files the snapshots leave out are kept.
    pub fn restore(&self, snapshot: &str) -> Result<String, String> {
        check(snapshot)?;
        let now = self.snapshot("before undo")?;
        self.git(&["read-tree", "-u", "--reset", snapshot])?;
        Ok(now)
    }
}

/// Only full snapshot ids reach git, never other revision words.
fn check(id: &str) -> Result<(), String> {
    if id.len() == 40 && id.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) {
        Ok(())
    } else {
        Err("not a checkpoint".into())
    }
}

/// The detail of a turn's changes row.
pub fn detail(before: &str, after: &str, files: &[Changed]) -> Value {
    json!({
        "kind": "changes",
        "before": before,
        "after": after,
        "files": files.iter().map(|f| json!({ "path": f.path, "how": f.how, "added": f.added, "removed": f.removed })).collect::<Vec<_>>(),
        "undone": false,
    })
}

/// The row's one-line summary: "Changed 3 files".
pub fn summary(files: &[Changed]) -> String {
    match files {
        [one] => format!("{} {}", match one.how {
            "added" => "Added",
            "deleted" => "Deleted",
            _ => "Changed",
        }, one.path),
        many => format!("Changed {} files", many.len()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_turn_is_seen_and_undone_without_touching_the_folders_own_repository() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("bot");
        std::fs::create_dir_all(folder.join(".git")).unwrap();
        std::fs::write(folder.join(".git/HEAD"), "the person's own").unwrap();
        std::fs::write(folder.join("plan.md"), "carrots\n").unwrap();
        std::fs::write(folder.join("old.txt"), "old\n").unwrap();
        let points = Checkpoints::new(dir.path(), "b1", &folder);

        let before = points.snapshot("before").unwrap();
        // The turn: a file changed, one added, one deleted, an attachment arrives.
        std::fs::write(folder.join("plan.md"), "carrots\nbeans\n").unwrap();
        std::fs::write(folder.join("new.md"), "hello\n").unwrap();
        std::fs::remove_file(folder.join("old.txt")).unwrap();
        std::fs::create_dir_all(folder.join("attachments")).unwrap();
        std::fs::write(folder.join("attachments/photo.png"), "png").unwrap();
        let after = points.snapshot("after").unwrap();

        let mut files = points.changes(&before, &after).unwrap();
        files.sort_by(|a, b| a.path.cmp(&b.path));
        let seen: Vec<(&str, &str, Option<u64>)> = files.iter().map(|f| (f.path.as_str(), f.how, f.added)).collect();
        assert_eq!(seen, vec![("new.md", "added", Some(1)), ("old.txt", "deleted", Some(0)), ("plan.md", "changed", Some(1))]);
        assert_eq!(summary(&files), "Changed 3 files");
        assert!(points.patch(&before, &after).unwrap().contains("+beans"));

        let undo_point = points.restore(&before).unwrap();
        assert_eq!(std::fs::read_to_string(folder.join("plan.md")).unwrap(), "carrots\n");
        assert_eq!(std::fs::read_to_string(folder.join("old.txt")).unwrap(), "old\n");
        assert!(!folder.join("new.md").exists());
        assert!(folder.join("attachments/photo.png").exists(), "attachments are left alone");
        assert_eq!(std::fs::read_to_string(folder.join(".git/HEAD")).unwrap(), "the person's own");
        // And the undo can be undone.
        points.restore(&undo_point).unwrap();
        assert!(folder.join("new.md").exists());

        assert!(points.patch("HEAD~1", &after).is_err());
    }
}
