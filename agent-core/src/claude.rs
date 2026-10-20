// Claude Code's own files.
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

use serde_json::Value;

use crate::s;
use crate::text::clip;

// How much of a transcript's end is read for its title: Claude Code writes
// the title again near the end as it goes.
const TITLE_TAIL: u64 = 256 * 1024;

// Claude Code's name for the session from its transcript: the last name the
// person (or the app) gave it, else the last one Claude made up.
pub fn transcript_title(path: &str) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(TITLE_TAIL))).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let (mut custom, mut ai) = (None, None);
    for line in text.lines().rev() {
        if custom.is_some() {
            break;
        }
        let is_custom = line.contains("\"custom-title\"");
        if !is_custom && (ai.is_some() || !line.contains("\"ai-title\"")) {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<Value>(line) else { continue };
        match s(&entry, "type") {
            "custom-title" => custom = Some(s(&entry, "customTitle").to_string()),
            "ai-title" => ai = Some(s(&entry, "aiTitle").to_string()),
            _ => {}
        }
    }
    custom.or(ai).map(|t| clip(&t, 80)).filter(|t| !t.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_s_name_from_its_transcript() {
        let dir = std::env::temp_dir().join(format!("agent-core-claude-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let transcript = dir.join("t.jsonl");
        let lines = [
            r#"{"type":"ai-title","aiTitle":"Made up","sessionId":"s"}"#,
            r#"{"type":"custom-title","customTitle":"Given","sessionId":"s"}"#,
            r#"{"type":"user","message":"hi"}"#,
            r#"{"type":"ai-title","aiTitle":"Made up later","sessionId":"s"}"#,
        ];
        std::fs::write(&transcript, lines.join("\n")).unwrap();
        assert_eq!(transcript_title(&transcript.to_string_lossy()).as_deref(), Some("Given"));
        std::fs::write(&transcript, [lines[0], lines[3]].join("\n")).unwrap();
        assert_eq!(transcript_title(&transcript.to_string_lossy()).as_deref(), Some("Made up later"));
        assert_eq!(transcript_title("Z:\\no\\such\\file.jsonl"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
