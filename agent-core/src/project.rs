// The project a session works in.
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use crate::text::base;

// The project a folder belongs to: the repository's own folder, so a session
// in a git worktree (.claude/worktrees/<name>, or anywhere) still says whose;
// outside git, the folder itself. Remembered per folder.
pub fn project_of(cwd: &str) -> String {
    static KNOWN: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);
    if let Some(name) = KNOWN.lock().unwrap().as_ref().and_then(|m| m.get(cwd)) {
        return name.clone();
    }
    let name = repo_name(Path::new(cwd)).unwrap_or_else(|| base(cwd).to_string());
    KNOWN.lock().unwrap().get_or_insert_with(HashMap::new).insert(cwd.to_string(), name.clone());
    name
}

fn repo_name(cwd: &Path) -> Option<String> {
    let name = |p: &Path| p.file_name().map(|n| n.to_string_lossy().to_string());
    for dir in cwd.ancestors().take(16) {
        let git = dir.join(".git");
        if git.is_dir() {
            return name(dir);
        }
        if git.is_file() {
            // A worktree's .git says where its repository keeps it:
            // gitdir: <repo>/.git/worktrees/<name>
            let text = std::fs::read_to_string(&git).ok()?;
            let gitdir = text.trim().strip_prefix("gitdir:")?.trim().replace('\\', "/");
            return match gitdir.find("/.git/worktrees/") {
                Some(at) => Some(base(&gitdir[..at]).to_string()),
                None => name(dir),
            };
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_s_project() {
        let dir = std::env::temp_dir().join(format!("agent-core-project-{}", std::process::id()));
        let wt = dir.join("repo").join(".claude").join("worktrees").join("brave-otter");
        std::fs::create_dir_all(dir.join("repo").join(".git")).unwrap();
        std::fs::create_dir_all(wt.join("src")).unwrap();
        let gitdir = dir.join("repo").join(".git").join("worktrees").join("brave-otter");
        std::fs::write(wt.join(".git"), format!("gitdir: {}\n", gitdir.display())).unwrap();
        assert_eq!(project_of(&wt.join("src").to_string_lossy()), "repo");
        assert_eq!(project_of(&dir.join("repo").to_string_lossy()), "repo");
        assert_eq!(project_of("Z:\\nowhere\\pet"), "pet");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
