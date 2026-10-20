// Text as the agents' events carry it.

// At most `max` characters, trimmed; a cut one ends in "…".
pub fn clip(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

// The last part of a path, either slash.
pub fn base(path: &str) -> &str {
    path.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or(path)
}

// A path an agent gave, whole: one relative to the folder it works in
// (`cwd`) joined to it, one whole (or with no folder to join it to) as it is.
pub fn absolute(cwd: &str, path: &str) -> String {
    if path.is_empty() || cwd.is_empty() || std::path::Path::new(path).is_absolute() {
        return path.to_string();
    }
    std::path::Path::new(cwd).join(path).to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_made_whole() {
        assert_eq!(absolute("C:\\w\\proj", "src\\a.rs"), "C:\\w\\proj\\src\\a.rs");
        assert_eq!(absolute("C:\\w\\proj", "C:\\other\\b.rs"), "C:\\other\\b.rs");
        assert_eq!((absolute("", "a.rs"), absolute("C:\\w", "")), ("a.rs".to_string(), String::new()));
    }
}
