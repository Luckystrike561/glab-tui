//! End-to-end coverage for `[[custom_keybindings.<pane>]]`: a key runs a
//! shell command with the highlighted row's fields, the app takes keys again
//! afterwards, a diff-view binding gets the file and line under the cursor,
//! and a key a built-in already owns keeps its built-in action.
//!
//! The mock `glab` serves `tests/fixtures/mrs.json` for `mr list`, whose one
//! MR is !2 `feature/pagination` → `main` by `test-user`, and
//! `custom_command_diff.txt` for `mr diff`. The harness pins
//! `SHELL=/bin/sh` so commands run under a known shell.

use crate::TestSession;
use std::path::Path;
use std::time::{Duration, Instant};

const MR_TITLE: &str = "Feature: Add pagination support";
const OUTPUT_FILE: &str = "custom_out.txt";
const EXPECTED_ROW_FIELDS: &str = "2|feature/pagination|main|test-user|test-owner/test-repo|2";

/// Launch with `config_toml` and switch to the MRs tab.
fn session_on_mrs_tab(config_toml: &str) -> TestSession {
    let mut session = TestSession::with_config(false, 40, 160, Some(config_toml));
    session
        .wait_for_screen_contains("Issues", 30000)
        .expect("app should reach the Issues tab");
    session.send_input(b"l");
    session
        .wait_for_screen_contains(MR_TITLE, 15000)
        .expect("MRs tab should list the fixture MR");
    session
}

fn wait_for_file(path: &Path, timeout_ms: u64) -> Result<String, String> {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    while Instant::now() < deadline {
        if let Ok(content) = std::fs::read_to_string(path) {
            if !content.is_empty() {
                return Ok(content);
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Err(format!("{} was never written", path.display()))
}

#[test]
fn test_mr_custom_command_runs_with_the_row_fields_and_resumes_the_tui() {
    let config = format!(
        r#"
[[custom_keybindings.mrs]]
key = "w"
name = "record pr"
command = "printf '%s|%s|%s|%s|%s|%s' {{{{.PrNumber}}}} {{{{.HeadRefName}}}} {{{{.BaseRefName}}}} {{{{.Author}}}} {{{{.RepoName}}}} \"$GLAB_TUI_PR_NUMBER\" > {{{{.RepoPath}}}}/{OUTPUT_FILE}"
"#
    );
    let session = session_on_mrs_tab(&config);
    let output_path = session.sandbox.repo_dir.join(OUTPUT_FILE);

    session.send_input(b"w");

    let output = wait_for_file(&output_path, 10000)
        .expect("the command should write into the repo checkout ({{.RepoPath}})");
    assert_eq!(output, EXPECTED_ROW_FIELDS);

    // Keys typed while the command owns the terminal are drained on resume,
    // so retry until the app takes the key again.
    std::fs::remove_file(&output_path).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !output_path.exists() {
        assert!(
            Instant::now() < deadline,
            "keypresses should reach the app again once the command has exited"
        );
        session.send_input(b"w");
        std::thread::sleep(Duration::from_millis(300));
    }
    assert_eq!(
        wait_for_file(&output_path, 5000).unwrap(),
        EXPECTED_ROW_FIELDS
    );
}

#[test]
fn test_custom_command_is_listed_in_help_on_its_tab() {
    let mut session = session_on_mrs_tab(
        r#"
[[custom_keybindings.mrs]]
key = "w"
name = "record pr"
command = "true"
"#,
    );

    session.send_input(b"?");

    session
        .wait_for_screen_contains("Custom Commands", 5000)
        .expect("help should have a custom commands section");
    session
        .wait_for_screen_contains("record pr", 5000)
        .expect("help should list the command by its name");
}

#[test]
fn test_failing_custom_command_raises_an_error_toast() {
    let mut session = session_on_mrs_tab(
        r#"
[[custom_keybindings.universal]]
key = "x"
name = "broken tool"
command = "exit 3"
"#,
    );

    session.send_input(b"x");

    session
        .wait_for_screen_contains("\"broken tool\" failed with exit status: 3", 10000)
        .expect("a non-zero exit should surface as an error toast");
}

#[test]
fn test_custom_binding_shadowed_by_a_builtin_never_runs() {
    let mut session = TestSession::with_config(
        false,
        40,
        160,
        Some(
            r#"
[[custom_keybindings.universal]]
key = "g"
command = "touch shadowed_ran"
"#,
        ),
    );
    session
        .wait_for_screen_contains("Issues", 30000)
        .expect("app should reach the Issues tab");

    session.send_input(b"g");

    session
        .wait_for_screen_contains("Jump to Issue/MR", 5000)
        .expect("g should still open the built-in jump selector");
    assert!(
        !session.sandbox.repo_dir.join("shadowed_ran").exists(),
        "the shadowed custom command must never run"
    );
}

#[test]
fn test_diff_custom_command_gets_the_file_and_line_under_the_cursor() {
    let config = format!(
        r#"
[[custom_keybindings.diff]]
key = "o"
name = "open in editor"
command = "printf '%s:%s %s' {{{{.FilePath}}}} {{{{.LineNumber}}}} {{{{.HeadRefName}}}} > {OUTPUT_FILE}"
"#
    );
    let mut session = session_on_mrs_tab(&config);
    session.send_input(b"D");
    session
        .wait_for_screen_contains("src/pagination.rs", 15000)
        .expect("D should open the MR diff with the fixture file");

    // Focus the diff pane and search; Esc leaves search with the cursor on
    // the match, the added line (new-side line 41).
    for key in b"\t/MAX_ROWS_QUIRK\x1b" {
        session.send_input(&[*key]);
        pump_output(&mut session, Duration::from_millis(150));
    }
    pump_output(&mut session, Duration::from_millis(300));
    session.send_input(b"o");

    let output = wait_for_file(&session.sandbox.repo_dir.join(OUTPUT_FILE), 10000)
        .expect("the diff binding should run from the diff view");
    assert_eq!(output, "src/pagination.rs:41 feature/pagination");
}

/// Waits between keystrokes while feeding the app's output to the emulator.
fn pump_output(session: &mut TestSession, duration: Duration) {
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline {
        let bytes = session.pty.read_output();
        if !bytes.is_empty() {
            session.emulator.write_bytes(&bytes);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn test_issue_custom_command_runs_once_per_selected_issue() {
    let config = format!(
        r#"
[[custom_keybindings.issues]]
key = "w"
name = "worktree per issue"
command = "printf '%s;' {{{{.IssueNumber}}}} >> {OUTPUT_FILE}"
"#
    );
    let mut session = TestSession::with_config(false, 40, 160, Some(&config));
    session
        .wait_for_screen_contains("Worktree for eight", 30000)
        .expect("the Issues tab should list both fixture issues");

    // Select both issues (Space on each), then run the command once.
    for key in b" j " {
        session.send_input(&[*key]);
        pump_output(&mut session, Duration::from_millis(150));
    }
    session.send_input(b"w");

    let path = session.sandbox.repo_dir.join(OUTPUT_FILE);
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut output = String::new();
    while Instant::now() < deadline && output != "7;8;" {
        pump_output(&mut session, Duration::from_millis(100));
        output = std::fs::read_to_string(&path).unwrap_or_default();
    }
    assert_eq!(
        output, "7;8;",
        "one run per selected issue, in number order"
    );
}

#[test]
fn test_background_custom_command_runs_without_the_terminal_and_reports_stderr() {
    let config = format!(
        r#"
[[custom_keybindings.mrs]]
key = "w"
name = "open pane"
background = true
command = "printf '%s' {{{{.PrNumber}}}} > {OUTPUT_FILE}"

[[custom_keybindings.mrs]]
key = "b"
name = "pane tool"
background = true
command = "echo 'herdr: no pane w9:p99' >&2; exit 2"
"#
    );
    let mut session = session_on_mrs_tab(&config);

    session.send_input(b"w");
    let output = wait_for_file(&session.sandbox.repo_dir.join(OUTPUT_FILE), 10000)
        .expect("the background command should run");
    assert_eq!(output, "2");

    session.send_input(b"b");
    session
        .wait_for_screen_contains("herdr: no pane w9:p99", 10000)
        .expect("a background failure should toast its stderr reason");
}
