use serde::Serialize;
use std::collections::HashMap;
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
fn output(path: &Path, args: &[&str]) -> Option<String> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
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
        let mut text = Vec::new();
        stdout.read_to_end(&mut text).map(|_| text)
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
    status.success().then(|| String::from_utf8_lossy(&text).into_owned())
}

pub fn read(path: &Path) -> Option<GitInfo> {
    output(path, &["status", "--porcelain=v2", "--branch", "--", "."])
        .map(|value| parse_status(&value))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChangedFile {
    path: String,
    status: String,
    added: Option<u32>,
    removed: Option<u32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Commit {
    subject: String,
    timestamp: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardGit {
    #[serde(flatten)]
    info: GitInfo,
    files: Vec<ChangedFile>,
    latest: Option<Commit>,
}

fn dashboard_status(text: &str) -> (GitInfo, Vec<ChangedFile>) {
    let mut headers = Vec::new();
    let mut files = Vec::new();
    let mut entries = text.split('\0');
    while let Some(entry) = entries.next() {
        if entry.starts_with("# ") {
            headers.push(entry);
            continue;
        }
        let Some((kind, remainder)) = entry.split_once(' ') else { continue; };
        let (status, path) = match kind {
            "?" => ("??", remainder),
            "1" | "2" | "u" => {
                let fields = match kind { "2" => 9, "u" => 10, _ => 8 };
                let mut parts = remainder.splitn(fields, ' ');
                let status = parts.next().unwrap_or_default();
                let Some(path) = parts.last() else { continue; };
                if kind == "2" { entries.next(); }
                let status = if status.contains('U') { "U" } else if status.contains('D') { "D" }
                    else if status.contains('A') { "A" } else if status.contains('R') { "R" } else { "M" };
                (status, path)
            }
            _ => continue,
        };
        files.push(ChangedFile { path: path.into(), status: status.into(), added: None, removed: None });
    }
    let mut info = parse_status(&headers.join("\n"));
    info.changes = files.len() as u32;
    (info, files)
}

fn numstat(text: &str) -> HashMap<String, (Option<u32>, Option<u32>)> {
    let mut entries = text.split('\0');
    let mut counts = HashMap::new();
    while let Some(line) = entries.next() {
        let mut fields = line.splitn(3, '\t');
        let (Some(added), Some(removed), Some(path)) = (fields.next(), fields.next(), fields.next()) else { continue; };
        let path = if path.is_empty() { entries.nth(1).unwrap_or_default() } else { path };
        counts.insert(path.into(), (added.parse().ok(), removed.parse().ok()));
    }
    counts
}

fn untracked_lines(root: &Path, path: &str) -> Option<u32> {
    let file = root.join(path);
    let metadata = file.symlink_metadata().ok()?;
    if !metadata.is_file() || metadata.len() > 512 * 1024 { return None; }
    let bytes = std::fs::read(file).ok()?;
    if bytes.contains(&0) { return None; }
    Some(std::str::from_utf8(&bytes).ok()?.lines().count() as u32)
}

fn dashboard(path: &Path) -> Option<DashboardGit> {
    let text = output(path, &["status", "--porcelain=v2", "--branch", "-z", "--untracked-files=all", "--", "."])?;
    let (info, mut files) = dashboard_status(&text);
    files.truncate(3);
    if !files.is_empty() {
        let args = if info.initial {
            ["diff", "--numstat", "-z", "--no-ext-diff", "--no-textconv", "--no-relative", "--cached", "--", "."]
        } else {
            ["diff", "--numstat", "-z", "--no-ext-diff", "--no-textconv", "--no-relative", "HEAD", "--", "."]
        };
        let counts = output(path, &args).map(|value| numstat(&value)).unwrap_or_default();
        let root = files.iter().any(|file| file.status == "??").then(||
            output(path, &["rev-parse", "--show-toplevel"])).flatten();
        for file in &mut files {
            if let Some(&(added, removed)) = counts.get(&file.path) {
                file.added = added;
                file.removed = removed;
            } else if file.status == "??" {
                file.added = root.as_deref().and_then(|root| untracked_lines(Path::new(root.trim_end_matches('\n')), &file.path));
                file.removed = file.added.map(|_| 0);
            }
        }
    }
    let latest = if info.initial { None } else {
        output(path, &["log", "-1", "--format=%ct%x00%s"]).and_then(|text| {
            let (timestamp, subject) = text.split_once('\0')?;
            Some(Commit { subject: subject.trim_end_matches('\n').into(), timestamp: timestamp.parse::<u64>().ok()? * 1000 })
        })
    };
    Some(DashboardGit { info, files, latest })
}

#[tauri::command]
pub async fn dashboard_git(id: String) -> Result<Option<DashboardGit>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let project = crate::get_projects_sync()?.projects.into_iter().find(|project| project.id == id).ok_or("未找到项目")?;
        Ok(dashboard(Path::new(&crate::expand_tilde(&project.path))))
    }).await.map_err(|error| format!("读取 Git 改动失败：{error}"))?
}

#[tauri::command]
pub async fn dashboard_open_editor(id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let project = crate::get_projects_sync()?.projects.into_iter().find(|project| project.id == id).ok_or("未找到项目")?;
        let path = crate::expand_tilde(&project.path);
        if !Path::new(&path).is_dir() { return Err("项目目录不存在".into()); }
        let preferred = std::env::var("BENTO_EDITOR").ok();
        let editors = ["Visual Studio Code", "Cursor", "Zed", "Windsurf", "Sublime Text"];
        for editor in preferred.as_deref().into_iter().chain(editors) {
            if Command::new("open").args(["-a", editor, &path]).output().is_ok_and(|result| result.status.success()) {
                return Ok(());
            }
        }
        Err("找不到可打开项目的编辑器。请安装支持的编辑器或设置 BENTO_EDITOR。".into())
    }).await.map_err(|error| format!("打开编辑器失败：{error}"))?
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

    #[test]
    fn dashboard_parses_nul_paths_and_rename_counts() {
        let (info, files) = dashboard_status(
            "# branch.head main\0# branch.ab +2 -1\0\
             1 .M N... 100644 100644 100644 a b with spaces.ts\0\
             2 R. N... 100644 100644 100644 a b R100 new\nname.ts\0old.ts\0\
             ? 中文/新文件.ts\0",
        );
        assert_eq!((info.ahead, info.behind, info.changes), (2, 1, 3));
        assert_eq!(files.iter().map(|file| (file.status.as_str(), file.path.as_str())).collect::<Vec<_>>(),
            [("M", "with spaces.ts"), ("R", "new\nname.ts"), ("??", "中文/新文件.ts")]);
        let counts = numstat("2\t1\twith spaces.ts\0-\t-\timage.png\01\t0\t\0old.ts\0new\nname.ts\0");
        assert_eq!(counts["with spaces.ts"], (Some(2), Some(1)));
        assert_eq!(counts["image.png"], (None, None));
        assert_eq!(counts["new\nname.ts"], (Some(1), Some(0)));
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
        let details = dashboard(&app).unwrap();
        assert_eq!(details.latest.unwrap().subject, "init");
        assert_eq!(details.files.len(), 2);
        assert_eq!((details.files[0].added, details.files[0].removed), (Some(1), Some(1)));
        assert_eq!((details.files[1].added, details.files[1].removed), (Some(1), Some(0)));

        let plain = root.join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        assert!(read(&plain).is_none());
        assert!(read(&root.join("missing")).is_none());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
