//! The "agent" half of an eval: build the prompt (skill + task + context) and
//! parse the model's file edits. Edit protocol is a wholesale file-block format
//! (robust to parse, no fuzzy diff application):
//!
//! ```text
//! <<<FILE: relative/path.ext>>>
//! <full new file content>
//! <<<END>>>
//! ```

use crate::config::Task;

pub struct Edit {
    pub path: String,
    pub content: String,
}

pub fn agent_system(skill: &str) -> String {
    format!(
        "You are a coding agent working inside a repository. Follow the project SKILL below \
precisely. For every file you change, output a block EXACTLY in this format:\n\
<<<FILE: relative/path.ext>>>\n<full new file content>\n<<<END>>>\n\
Output ONLY such file blocks — no prose, no markdown fences.\n\n\
=== PROJECT SKILL ===\n{skill}\n=== END SKILL ==="
    )
}

pub fn agent_user(task: &Task, ctx: &[(String, String)]) -> String {
    let mut s = String::new();
    s.push_str(&format!("TASK: {}\n\n", task.intent));
    if !task.target_files.is_empty() {
        s.push_str(&format!(
            "Write these target file(s): {}\n\n",
            task.target_files.join(", ")
        ));
    }
    for (path, content) in ctx {
        s.push_str(&format!("--- current file: {path} ---\n{content}\n\n"));
    }
    s
}

/// Parse `<<<FILE: path>>> ... <<<END>>>` blocks into edits.
pub fn parse_edits(response: &str) -> Vec<Edit> {
    let mut edits = Vec::new();
    let mut lines = response.lines();
    while let Some(line) = lines.next() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("<<<FILE:") {
            let path = rest.trim_end_matches(">>>").trim().to_string();
            let mut content = String::new();
            for l in lines.by_ref() {
                if l.trim() == "<<<END>>>" {
                    break;
                }
                content.push_str(l);
                content.push('\n');
            }
            if !path.is_empty() {
                edits.push(Edit { path, content });
            }
        }
    }
    edits
}
