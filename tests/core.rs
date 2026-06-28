//! Integration tests over the public library API — no network / LLM. Exercises
//! `parse_edits`, project discovery, and the full agent -> ExecJudge -> eval
//! pipeline against a temporary git repo (mock provider, grep-based verify).

use skillsmith::agent::parse_edits;
use skillsmith::config::{self, Project, ProjectConfig, Task};
use skillsmith::eval::{EvalReport, eval_skill};
use skillsmith::judge::ExecJudge;
use skillsmith::llm::LlmProvider;
use skillsmith::obs::Emitter;
use std::path::Path;
use std::process::Command;

#[test]
fn parse_edits_extracts_file_blocks() {
    let resp = "noise\n<<<FILE: a/b.py>>>\nline1\nline2\n<<<END>>>\ntrailing";
    let edits = parse_edits(resp);
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0].path, "a/b.py");
    assert_eq!(edits[0].content, "line1\nline2\n");
}

#[test]
fn list_projects_discovers_configs() {
    let home = tempfile::tempdir().unwrap();
    let pdir = home.path().join("projects").join("foo");
    std::fs::create_dir_all(&pdir).unwrap();
    std::fs::write(
        pdir.join("config.toml"),
        "name = \"foo\"\nrepo_path = \".\"\nskill_file = \"skill.md\"\n",
    )
    .unwrap();
    let found = config::list_projects(home.path()).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].name, "foo");
    assert_eq!(found[0].tasks, 0);
}

/// Mock provider — returns a canned response, no network.
struct MockLlm {
    response: String,
}
impl LlmProvider for MockLlm {
    async fn complete(&self, _model: &str, _system: &str, _user: &str) -> anyhow::Result<String> {
        Ok(self.response.clone())
    }
}

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .current_dir(dir)
        .args(args)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?} failed");
}

fn make_project(repo: &Path) -> Project {
    let task = Task {
        id: "magic".into(),
        intent: "write the magic file".into(),
        context_files: vec![],
        target_files: vec!["out.txt".into()],
        holdout: false,
        split: None,
        setup_cmd: String::new(),
        verify_cmd: "grep -q SKILLSMITH_OK out.txt".into(),
    };
    let cfg = ProjectConfig {
        name: "t".into(),
        repo_path: repo.to_string_lossy().into_owned(),
        skill_file: "skill.md".into(),
        agent_model: "mock".into(),
        optimizer_model: "mock".into(),
        provider: "claude".into(),
        provider_cmd: vec![],
        agent_provider_cmd: vec![],
        optimizer_provider_cmd: vec![],
        rounds: 1,
        tasks: vec![task],
        deploy: Default::default(),
    };
    Project {
        dir: repo.to_path_buf(),
        cfg,
    }
}

#[tokio::test]
async fn exec_judge_grades_pass_and_fail_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    std::fs::write(repo.join("seed.txt"), "seed\n").unwrap();
    git(repo, &["init", "-q"]);
    git(repo, &["add", "-A"]);
    git(
        repo,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "-m",
            "init",
        ],
    );

    let project = make_project(repo);

    // PASS: the agent writes the magic content -> verify_cmd (grep) exits 0.
    let good = MockLlm {
        response: "<<<FILE: out.txt>>>\nSKILLSMITH_OK\n<<<END>>>".into(),
    };
    let rep = eval_skill(&good, &ExecJudge, &project, "skill", &project.cfg.tasks, &Emitter::silent())
        .await
        .unwrap();
    assert_eq!(rep.score(), 1.0, "valid edit should pass the verify_cmd");

    // FAIL: wrong content -> grep exits non-zero.
    let bad = MockLlm {
        response: "<<<FILE: out.txt>>>\nNOPE\n<<<END>>>".into(),
    };
    let rep2 = eval_skill(&bad, &ExecJudge, &project, "skill", &project.cfg.tasks, &Emitter::silent())
        .await
        .unwrap();
    assert_eq!(rep2.score(), 0.0, "wrong content should fail");
}

#[test]
fn adopt_copies_staged_over_live_and_clears_it() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    std::fs::write(repo.join("skill.md"), "OLD").unwrap();
    std::fs::write(repo.join("skill.staged.md"), "NEW").unwrap();
    let project = make_project(repo); // skill_file = "skill.md", dir = repo

    let (staged, live) = skillsmith::optimize::adopt_project(&project).unwrap();
    assert_eq!(
        std::fs::read_to_string(&live).unwrap(),
        "NEW",
        "live skill is replaced by the staged proposal"
    );
    assert!(!staged.exists(), "staged file is cleared after adoption");

    // No staged proposal left -> a second adopt errors (nothing to adopt).
    assert!(skillsmith::optimize::adopt_project(&project).is_err());
}

#[test]
fn init_seeds_demo_with_git_fixture() {
    let home = tempfile::tempdir().unwrap();
    skillsmith::seed::init(home.path()).unwrap();
    let demo = home.path().join("projects").join("demo");
    assert!(demo.join("config.toml").is_file());
    assert!(demo.join("skill.md").is_file());
    // The fixture must be a real git repo so ExecJudge can `git worktree` it.
    assert!(demo.join("fixture").join(".git").exists());
    assert!(demo.join("fixture").join("test_strings.py").is_file());
    let found = config::list_projects(home.path()).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].name, "demo");
    assert_eq!(found[0].tasks, 2);
}

#[test]
fn project_repo_resolves_relative_to_project_dir() {
    let home = tempfile::tempdir().unwrap();
    let pdir = home.path().join("projects").join("demo");
    std::fs::create_dir_all(pdir.join("fixture")).unwrap();
    let cfg = ProjectConfig {
        name: "demo".into(),
        repo_path: "fixture".into(), // relative -> resolves against the project dir
        skill_file: "skill.md".into(),
        agent_model: "mock".into(),
        optimizer_model: "mock".into(),
        provider: "claude".into(),
        provider_cmd: vec![],
        agent_provider_cmd: vec![],
        optimizer_provider_cmd: vec![],
        rounds: 1,
        tasks: vec![],
        deploy: Default::default(),
    };
    let project = Project {
        dir: pdir.clone(),
        cfg,
    };
    let expected = std::fs::canonicalize(pdir.join("fixture")).unwrap();
    assert_eq!(project.repo().unwrap(), expected);
}

#[test]
fn discover_dot_skillsmith_finds_nearest_ancestor() {
    use skillsmith::config::discover_dot_skillsmith;
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    let deep = repo.join("a").join("b").join("c");
    std::fs::create_dir_all(&deep).unwrap();
    // No .skillsmith anywhere yet -> None.
    assert!(discover_dot_skillsmith(&deep).is_none());
    // Put one at the repo root; a deep cwd walks up and finds it.
    std::fs::create_dir(repo.join(".skillsmith")).unwrap();
    assert_eq!(
        discover_dot_skillsmith(&deep).unwrap(),
        repo.join(".skillsmith")
    );
    // A nearer one wins over the farther one.
    std::fs::create_dir(repo.join("a").join(".skillsmith")).unwrap();
    assert_eq!(
        discover_dot_skillsmith(&deep).unwrap(),
        repo.join("a").join(".skillsmith")
    );
}

#[test]
fn slug_from_path_derives_project_name() {
    use skillsmith::config::slug_from_path;
    assert_eq!(
        slug_from_path(Path::new("/a/b/llamon-agent-sdk")).as_deref(),
        Some("llamon-agent-sdk")
    );
    assert_eq!(slug_from_path(Path::new("/x/My Repo!")).as_deref(), Some("my-repo"));
    assert_eq!(slug_from_path(Path::new("/x/.hidden")).as_deref(), Some("hidden"));
    assert_eq!(slug_from_path(Path::new("/x/123_proj")).as_deref(), Some("123-proj"));
    assert_eq!(slug_from_path(Path::new("/x/---")), None); // all separators -> no slug
}

#[test]
fn repo_path_omitted_defaults_to_enclosing_git_root() {
    // Repo-local layout: <repo>/.skillsmith/projects/p with an empty repo_path.
    let repo = tempfile::tempdir().unwrap();
    std::fs::create_dir(repo.path().join(".git")).unwrap(); // marks the repo root
    let pdir = repo.path().join(".skillsmith").join("projects").join("p");
    std::fs::create_dir_all(&pdir).unwrap();
    let cfg = ProjectConfig {
        name: "p".into(),
        repo_path: String::new(), // omitted -> enclosing git root
        skill_file: "skill.md".into(),
        agent_model: "mock".into(),
        optimizer_model: "mock".into(),
        provider: "claude".into(),
        provider_cmd: vec![],
        agent_provider_cmd: vec![],
        optimizer_provider_cmd: vec![],
        rounds: 1,
        tasks: vec![],
        deploy: Default::default(),
    };
    let project = Project { dir: pdir, cfg };
    assert_eq!(project.repo().unwrap(), repo.path());

    // And with no enclosing .git, an omitted repo_path is a clear error, not a guess.
    let orphan = tempfile::tempdir().unwrap();
    let odir = orphan.path().join("projects").join("p");
    std::fs::create_dir_all(&odir).unwrap();
    let ocfg = ProjectConfig {
        name: "p".into(),
        repo_path: String::new(),
        skill_file: "skill.md".into(),
        agent_model: "mock".into(),
        optimizer_model: "mock".into(),
        provider: "claude".into(),
        provider_cmd: vec![],
        agent_provider_cmd: vec![],
        optimizer_provider_cmd: vec![],
        rounds: 1,
        tasks: vec![],
        deploy: Default::default(),
    };
    assert!(Project { dir: odir, cfg: ocfg }.repo().is_err());
}

#[test]
fn scaffold_local_omits_repo_path_and_writes_gitignore() {
    let home = tempfile::tempdir().unwrap();
    let dir = config::scaffold_project(home.path(), "p", None, true).unwrap();
    let cfg = std::fs::read_to_string(dir.join("config.toml")).unwrap();
    assert!(cfg.contains("repo_path = \"\""), "repo_path left blank for local");
    assert!(dir.join("skill.md").is_file());
    // The scratch .gitignore keeps generated artifacts out of git.
    let gi = std::fs::read_to_string(home.path().join(".gitignore")).unwrap();
    assert!(gi.contains("skill.staged.md"));
    assert!(gi.contains("report.md"));
    // The omitted repo_path must round-trip through the config parser as empty.
    let loaded = Project::load(home.path(), "p").unwrap();
    assert!(loaded.cfg.repo_path.is_empty());
}

#[test]
fn grade_parses_runner_partial_credit() {
    use skillsmith::judge::grade;
    // pytest summary -> 3 of 4
    assert!((grade(1, "==== 3 passed, 1 failed in 0.12s ====") - 0.75).abs() < 1e-9);
    // unittest summary -> 3 of 4
    assert!((grade(1, "Ran 4 tests in 0.0s\n\nFAILED (failures=1)") - 0.75).abs() < 1e-9);
    // all green
    assert_eq!(grade(0, "Ran 2 tests in 0.0s\n\nOK"), 1.0);
    // no parseable count -> binary fallback on the exit code
    assert_eq!(grade(0, "build succeeded"), 1.0);
    assert_eq!(grade(1, "boom"), 0.0);
}

#[test]
fn gate_score_uses_held_out_tasks_only() {
    use skillsmith::judge::Outcome;
    use std::collections::HashSet;
    let report = EvalReport {
        outcomes: vec![
            Outcome {
                id: "train".into(),
                passed: false,
                score: 0.0,
                detail: String::new(),
            },
            Outcome {
                id: "val".into(),
                passed: true,
                score: 1.0,
                detail: String::new(),
            },
        ],
    };
    let holdout: HashSet<String> = ["val".to_string()].into_iter().collect();
    assert_eq!(report.gate_score(&holdout), 1.0); // only the val task gates
    assert_eq!(report.gate_score(&HashSet::new()), 0.5); // empty -> all tasks
}

#[test]
fn task_split_resolves_holdout_and_explicit() {
    use skillsmith::config::{ProjectConfig, TaskSplit};
    let toml = r#"
name = "t"
skill_file = "skill.md"
[[task]]
id = "train_default"
intent = "x"
verify_cmd = "true"
[[task]]
id = "val_via_holdout"
intent = "x"
verify_cmd = "true"
holdout = true
[[task]]
id = "test_explicit"
intent = "x"
verify_cmd = "true"
split = "test"
[[task]]
id = "split_wins_over_holdout"
intent = "x"
verify_cmd = "true"
holdout = true
split = "train"
"#;
    let cfg: ProjectConfig = toml::from_str(toml).unwrap();
    assert_eq!(cfg.tasks[0].split(), TaskSplit::Train, "default -> train");
    assert_eq!(cfg.tasks[1].split(), TaskSplit::Val, "holdout=true -> val (back-compat)");
    assert_eq!(cfg.tasks[2].split(), TaskSplit::Test, "explicit split wins");
    assert_eq!(cfg.tasks[3].split(), TaskSplit::Train, "explicit split overrides holdout");
}

#[test]
fn answer_leak_warning_flags_test_in_context_files() {
    use skillsmith::optimize::answer_leak_warnings;
    // Clean: empty context_files -> no warning (this is the held-out norm).
    let clean = make_project(Path::new("/tmp/x"));
    assert!(answer_leak_warnings(&clean).is_empty());

    // Leak: the verify_cmd's test file is also handed to the agent as context.
    let mut leaky = make_project(Path::new("/tmp/x"));
    leaky.cfg.tasks[0].verify_cmd = "pytest tests/test_thing.py -q".into();
    leaky.cfg.tasks[0].context_files = vec!["tests/test_thing.py".into()];
    let warns = answer_leak_warnings(&leaky);
    assert_eq!(warns.len(), 1);
    assert!(warns[0].contains("test_thing.py"));
}

#[tokio::test]
async fn dry_run_executes_verify_without_llm() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    std::fs::write(repo.join("seed.txt"), "seed\n").unwrap();
    git(repo, &["init", "-q"]);
    git(repo, &["add", "-A"]);
    git(
        repo,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "-m",
            "init",
        ],
    );
    let project = make_project(repo);
    // No LlmProvider involved — dry-run drives only the judge. A failing verify
    // (grep on a missing file) still counts as "ran" (no internal error).
    let ok = skillsmith::optimize::dry_run_project(&ExecJudge, &project).await;
    assert!(ok, "verify_cmd ran without an internal error");
}

#[test]
fn wrap_skill_emits_name_and_quoted_description_frontmatter() {
    use skillsmith::deploy::wrap_skill;
    let s = wrap_skill("orchestrator-target", "Use when \"x\" happens", "# body\n- rule");
    assert!(s.starts_with("---\nname: orchestrator-target\n"), "name in frontmatter");
    // description is YAML double-quoted with inner quotes escaped.
    assert!(s.contains("description: \"Use when \\\"x\\\" happens\"\n"));
    assert!(s.contains("---\n\n# body\n- rule\n"), "body follows the frontmatter");
}

#[test]
fn inject_context_appends_then_updates_idempotently() {
    use skillsmith::deploy::inject_context;
    let first = inject_context("# CLAUDE\n\nexisting rules", "demo", "RULES v1");
    assert!(first.contains("<!-- skillsmith:demo START -->"));
    assert!(first.contains("RULES v1"));
    assert!(first.contains("existing rules"), "pre-existing content is preserved");

    // Re-deploy with a new body replaces between the markers — no duplicate block.
    let second = inject_context(&first, "demo", "RULES v2");
    assert_eq!(
        second.matches("<!-- skillsmith:demo START -->").count(),
        1,
        "re-deploy is idempotent (one block, updated in place)"
    );
    assert!(second.contains("RULES v2"));
    assert!(!second.contains("RULES v1"));
    assert!(second.contains("existing rules"));
}
