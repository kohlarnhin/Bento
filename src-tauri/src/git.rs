use serde::Serialize;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitInfo {
    /// None while HEAD is detached.
    branch: Option<String>,
    head: Option<String>,
    initial: bool,
    upstream: Option<String>,
    ahead: u32,
    behind: u32,
    changes: u32,
}

/// Parses `git status --porcelain=v2 --branch` output.
pub fn parse_status(text: &str) -> GitInfo {
    let mut info = GitInfo::default();
    for line in text.lines() {
        if let Some(header) = line.strip_prefix("# ") {
            let (key, value) = header.split_once(' ').unwrap_or((header, ""));
            match key {
                "branch.oid" if value == "(initial)" => info.initial = true,
                "branch.oid" => info.head = Some(value.chars().take(7).collect()),
                "branch.head" if value != "(detached)" => info.branch = Some(value.into()),
                "branch.upstream" => info.upstream = Some(value.into()),
                "branch.ab" => {
                    for part in value.split_whitespace() {
                        if let Some(count) = part.strip_prefix('+') {
                            info.ahead = count.parse().unwrap_or(0);
                        } else if let Some(count) = part.strip_prefix('-') {
                            info.behind = count.parse().unwrap_or(0);
                        }
                    }
                }
                _ => {}
            }
        } else if !line.is_empty() {
            info.changes += 1;
        }
    }
    info
}

/// Reads branch and change counts without taking git's index lock; None when the
/// directory isn't inside a repository, git is missing, or it doesn't answer in time.
pub fn read(path: &Path) -> Option<GitInfo> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["status", "--porcelain=v2", "--branch", "--", "."])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    // Drain stdout on a thread so a large status can't fill the pipe and stall git.
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        stdout.read_to_string(&mut text).map(|_| text)
    });
    let deadline = Instant::now() + TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    let text = reader.join().ok()?.ok()?;
    status.success().then(|| parse_status(&text))
}

#[tauri::command]
pub async fn git_info(id: String) -> Result<Option<GitInfo>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let project = crate::get_projects_sync()?
            .projects
            .into_iter()
            .find(|project| project.id == id)
            .ok_or("未找到项目")?;
        Ok(read(Path::new(&crate::expand_tilde(&project.path))))
    })
    .await
    .map_err(|err| format!("读取 Git 状态失败：{err}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_branch_upstream_divergence_and_changes() {
        let info = parse_status(
            "# branch.oid 0123456789abcdef\n# branch.head main\n# branch.upstream origin/main\n# branch.ab +2 -1\n1 .M N... 100644 100644 100644 a b src/a.ts\n? new.txt\n",
        );
        assert_eq!(
            info,
            GitInfo {
                branch: Some("main".into()),
                head: Some("0123456".into()),
                initial: false,
                upstream: Some("origin/main".into()),
                ahead: 2,
                behind: 1,
                changes: 2,
            }
        );
    }

    #[test]
    fn parses_detached_and_initial_heads() {
        let detached = parse_status("# branch.oid abcdef0123\n# branch.head (detached)\n");
        assert_eq!(detached.branch, None);
        assert_eq!(detached.head.as_deref(), Some("abcdef0"));
        let initial = parse_status("# branch.oid (initial)\n# branch.head main\n? a\n");
        assert!(initial.initial && initial.head.is_none());
        assert_eq!((initial.branch.as_deref(), initial.changes), (Some("main"), 1));
    }

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    #[test]
    fn reads_real_repositories_scoped_to_the_project_directory() {
        let root = std::env::temp_dir().join(format!("apphub-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let repo = root.join("repo 中文");
        let app = repo.join("apps/web");
        std::fs::create_dir_all(&app).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        for (key, value) in [("user.email", "t@example.com"), ("user.name", "t"), ("commit.gpgsign", "false")] {
            git(&repo, &["config", key, value]);
        }
        std::fs::write(app.join("a.txt"), "a").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "init"]);

        let clean = read(&app).unwrap();
        assert_eq!((clean.branch.as_deref(), clean.changes, clean.initial), (Some("main"), 0, false));
        // A change elsewhere in the repository isn't this project's change.
        std::fs::write(repo.join("outside.txt"), "x").unwrap();
        std::fs::write(app.join("a.txt"), "changed").unwrap();
        std::fs::write(app.join("b.txt"), "new").unwrap();
        assert_eq!(read(&app).unwrap().changes, 2);
        assert_eq!(read(&repo).unwrap().changes, 3);

        let plain = root.join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        assert!(read(&plain).is_none());
        assert!(read(&root.join("missing")).is_none());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
