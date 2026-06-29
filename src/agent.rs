//! eval의 "에이전트" 절반: 프롬프트(스킬 + 태스크 + 컨텍스트)를 구성하고
//! 모델의 파일 편집 결과를 파싱한다. 편집 프로토콜은 파일 전체를 교체하는 블록 형식
//! (파싱이 단순하고 퍼지 diff 적용이 불필요):
//!
//! ```text
//! <<<FILE: relative/path.ext>>>
//! <전체 새 파일 내용>
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

/// `<<<FILE: path>>> ... <<<END>>>` 블록을 파싱해서 편집 목록으로 변환한다.
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
