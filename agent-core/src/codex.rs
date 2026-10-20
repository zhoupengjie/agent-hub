// Codex's edits: every one is apply_patch, the patch its tool_input.command
// (OpenCode's patch tool takes the same format).

// The files a patch adds, changes or deletes.
pub fn patched(patch: &str) -> Vec<&str> {
    patch
        .lines()
        .filter_map(|line| ["*** Add File: ", "*** Update File: ", "*** Delete File: "].iter().find_map(|p| line.strip_prefix(p)))
        .map(str::trim)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_files_a_patch_touches() {
        let patch = "*** Begin Patch\n*** Add File: hello.txt\n+hi\n*** Update File: D:\\w\\b.txt\n@@\n-a\n+b\n*** Delete File: old.txt\n*** End Patch";
        assert_eq!(patched(patch), ["hello.txt", "D:\\w\\b.txt", "old.txt"]);
        assert!(patched("not a patch").is_empty());
    }
}
