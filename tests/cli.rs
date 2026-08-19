use std::fs;
#[cfg(unix)]
use std::path::Path;
#[cfg(target_os = "macos")]
use std::process::Child;
use std::process::{Command, Output, Stdio};
#[cfg(target_os = "macos")]
use std::thread;
#[cfg(target_os = "macos")]
use std::time::{Duration, Instant};

use tempfile::TempDir;
#[cfg(target_os = "macos")]
use zwirn::adls::{Document, NodeKind};

const REPRESENTATIVE: &[u8] = include_bytes!("fixtures/representative.audulus4");
const EMPTY: &[u8] = include_bytes!("fixtures/empty.audulus4");
const LUA: &[u8] = include_bytes!("fixtures/angular_smoother.lua");
const LYTE: &[u8] = include_bytes!("fixtures/angular_smoother.lyte");

#[cfg(target_os = "macos")]
const LIVE_DEADLINE: Duration = Duration::from_secs(20);
#[cfg(target_os = "macos")]
const LIVE_POLL_INTERVAL: Duration = Duration::from_millis(25);
#[cfg(target_os = "macos")]
const SAVED_LIVE_SOURCE: &[u8] = b"saved live source\n";
#[cfg(target_os = "macos")]
const RECOVERED_LIVE_SOURCE: &[u8] = b"recovered live source\n";
#[cfg(target_os = "macos")]
const BLOCKER_WITNESS: &[u8] = b"blocker witness\n";

#[test]
fn root_help_explains_shared_operands() {
    let workspace = tempfile::tempdir().unwrap();
    let help = run(&workspace, ["--help"]);

    assert_eq!(help.status.code(), Some(0));
    assert!(help.stderr.is_empty());

    let stdout = String::from_utf8_lossy(&help.stdout);
    assert!(stdout.contains("Usage: zwirn <COMMAND>"));
    assert!(stdout.contains("DOCUMENT is an .audulus4 file."));
    assert!(stdout.contains(
        "For one-shot commands, FRAGMENT is an exact marker path relative to the source root"
    ));
    assert!(stdout.contains("zwirn status patch.audulus4"));
    assert!(stdout.contains("zwirn live patch.audulus4"));
}

#[cfg(target_os = "linux")]
#[test]
fn live_is_visible_but_unsupported_on_linux() {
    let workspace = representative_workspace();
    let help = run(&workspace, ["live", "--help"]);

    assert_eq!(help.status.code(), Some(0));
    assert!(help.stderr.is_empty());
    let stdout = String::from_utf8_lossy(&help.stdout);
    assert!(stdout.contains("Usage: zwirn live"));
    assert!(stdout.contains("<DOCUMENT>"));
    assert!(!stdout.contains("FRAGMENT"));

    let live = run(&workspace, ["live", "patch.audulus4"]);
    assert_eq!(live.status.code(), Some(2));
    assert!(live.stdout.is_empty());
    assert!(stderr(&live).contains("unsupported"));
}

#[cfg(target_os = "macos")]
#[test]
fn live_rejects_a_non_document_extension_before_session_startup() {
    let workspace = tempfile::tempdir().unwrap();
    fs::write(workspace.path().join("patch.txt"), REPRESENTATIVE).unwrap();

    let child = command(&workspace)
        .args(["live", "patch.txt"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut live = LiveChild::new(child);
    wait_for_live_exit(&mut live, "reject the non-document path");

    let output = live.wait_with_output();
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    assert!(output.stdout.is_empty());
    let diagnostics = stderr(&output);
    assert!(diagnostics.starts_with("zwirn: "));
    assert!(diagnostics.contains(".audulus4"));
    assert!(
        !diagnostics
            .lines()
            .any(|line| line.starts_with("zwirn live:")),
        "session setup produced a live diagnostic:\n{diagnostics}"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn live_reconciles_recovers_from_a_blocker_and_stops_on_sigterm() {
    use std::os::unix::fs::symlink;

    let workspace = tempfile::tempdir().unwrap();
    let document = workspace.path().join("patch.audulus4");
    let source_root = workspace.path().join("source");
    fs::create_dir(&source_root).unwrap();
    fs::create_dir(source_root.join("real")).unwrap();
    symlink("real", source_root.join("alias")).unwrap();
    fs::write(&document, REPRESENTATIVE).unwrap();

    let child = command(&workspace)
        .args(["live", "--source-root", "source", "patch.audulus4"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut live = LiveChild::new(child);

    wait_for_live_consequence(&mut live, "complete its initial reconciliation", || {
        fs::read(source_root.join("angular_smoother.lua")).is_ok_and(|bytes| bytes == LUA)
            && fs::read(source_root.join("angular_smoother.lyte")).is_ok_and(|bytes| bytes == LYTE)
            && live_inventory_is_synchronized(&workspace)
    });

    let initially_adopted = fs::read(&document).unwrap();
    fs::write(source_root.join("angular_smoother.lua"), SAVED_LIVE_SOURCE).unwrap();
    wait_for_live_consequence(&mut live, "reconcile a saved source change", || {
        live_inventory_is_synchronized(&workspace)
            && fs::read(&document).is_ok_and(|bytes| bytes != initially_adopted)
    });

    let adopted_after_saved_change = fs::read(&document).unwrap();
    let blocked_document = document_with_live_blocker(&adopted_after_saved_change);
    replace_document(&document, &blocked_document);
    // The two absent marker paths resolve through the relative directory
    // symlink to one target. Ordered commit leaves this first write behind as
    // a durable witness before its post-create alias check blocks the run.
    wait_for_live_consequence(&mut live, "reach the reconciliation blocker", || {
        fs::read(source_root.join("real/live-blocker.lua"))
            .is_ok_and(|bytes| bytes == BLOCKER_WITNESS)
    });

    fs::write(
        source_root.join("angular_smoother.lua"),
        RECOVERED_LIVE_SOURCE,
    )
    .unwrap();
    replace_document(&document, &adopted_after_saved_change);
    wait_for_live_consequence(&mut live, "recover on a later filesystem hint", || {
        fs::read(source_root.join("angular_smoother.lua"))
            .is_ok_and(|bytes| bytes == RECOVERED_LIVE_SOURCE)
            && live_inventory_is_synchronized(&workspace)
            && fs::read(&document).is_ok_and(|bytes| bytes != adopted_after_saved_change)
    });

    let pid = live.id().to_string();
    let signal = Command::new("/bin/kill")
        .args(["-TERM", pid.as_str()])
        .status()
        .unwrap();
    assert!(signal.success(), "failed to deliver SIGTERM: {signal}");
    wait_for_live_exit(&mut live, "stop after SIGTERM");

    let output = live.wait_with_output();
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(output.stdout.is_empty());
    let diagnostics = stderr(&output);
    assert_live_diagnostic_category(&diagnostics, "monitoring");
    assert!(
        diagnostics.lines().any(|line| {
            line.rsplit_once('\t')
                .is_some_and(|(_, result)| matches!(result, "record" | "embed" | "extract"))
        }),
        "live emitted no action diagnostic:\n{diagnostics}"
    );
    assert_live_diagnostic_category(&diagnostics, "external file already written");
    assert_live_diagnostic_category(&diagnostics, "blocked:");
    assert_live_diagnostic_category(&diagnostics, "reconciliation recovered");
    assert_live_diagnostic_category(&diagnostics, "stopped");
}

#[test]
fn status_and_sync_form_a_sorted_idempotent_workflow() {
    let workspace = representative_workspace();
    let source_root = workspace.path().join("source");
    fs::create_dir(&source_root).unwrap();
    fs::write(source_root.join("angular_smoother.lua"), LUA).unwrap();

    let status = run(
        &workspace,
        ["status", "--source-root", "source", "patch.audulus4"],
    );
    assert_result(
        &status,
        1,
        concat!(
            "angular_smoother.lua\tunadopted\n",
            "angular_smoother.lyte\tunadopted\n",
        ),
        "",
    );

    let sync = run(
        &workspace,
        ["sync", "--source-root", "source", "patch.audulus4"],
    );
    assert_result(
        &sync,
        0,
        concat!(
            "angular_smoother.lua\trecord\n",
            "angular_smoother.lyte\textract\n",
        ),
        "",
    );
    assert_eq!(
        fs::read(source_root.join("angular_smoother.lyte")).unwrap(),
        LYTE
    );

    let document = workspace.path().join("patch.audulus4");
    let adopted = fs::read(&document).unwrap();

    let second = run(
        &workspace,
        ["sync", "--source-root", "source", "patch.audulus4"],
    );
    assert_result(&second, 0, "", "");
    assert_eq!(fs::read(&document).unwrap(), adopted);
}

#[test]
fn selector_and_force_validation_are_command_errors() {
    let workspace = representative_workspace();

    let selector = run(&workspace, ["status", "patch.audulus4", "../bad.lua"]);
    assert_eq!(selector.status.code(), Some(2));
    assert!(selector.stdout.is_empty());
    assert!(stderr(&selector).contains("a fragment path contains a `..` segment"));

    let force = run(&workspace, ["embed", "--force", "patch.audulus4"]);
    assert_result(
        &force,
        2,
        "",
        "zwirn: --force requires at least one explicitly selected fragment\n",
    );
}

#[test]
fn forced_embed_uses_the_selected_filesystem_source() {
    let workspace = representative_workspace();
    fs::write(
        workspace.path().join("angular_smoother.lua"),
        b"filesystem wins\n",
    )
    .unwrap();

    let forced = run(
        &workspace,
        ["embed", "--force", "patch.audulus4", "angular_smoother.lua"],
    );
    assert_result(&forced, 0, "angular_smoother.lua\tembed\n", "");

    let status = run(
        &workspace,
        ["status", "patch.audulus4", "angular_smoother.lua"],
    );
    assert_result(&status, 0, "angular_smoother.lua\tsynchronized\n", "");
}

#[cfg(unix)]
#[test]
fn a_broken_stdout_is_an_operational_error() {
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixStream;

    let workspace = representative_workspace();
    let (reader, writer) = UnixStream::pair().unwrap();
    drop(reader);
    let writer = Stdio::from(OwnedFd::from(writer));

    let output = command(&workspace)
        .args(["status", "patch.audulus4"])
        .stdout(writer)
        .stderr(Stdio::piped())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).starts_with("zwirn: cannot write command output:"));
}

#[test]
fn an_empty_inventory_is_already_synchronized() {
    let workspace = tempfile::tempdir().unwrap();
    let document = workspace.path().join("empty.audulus4");
    fs::write(&document, EMPTY).unwrap();

    let status = run(&workspace, ["status", "empty.audulus4"]);
    assert_result(&status, 0, "", "");
    let sync = run(&workspace, ["sync", "empty.audulus4"]);
    assert_result(&sync, 0, "", "");
    assert_eq!(fs::read(document).unwrap(), EMPTY);
}

#[cfg(unix)]
#[test]
fn default_source_root_is_the_named_document_parent() {
    use std::os::unix::fs::symlink;

    let workspace = tempfile::tempdir().unwrap();
    fs::create_dir(workspace.path().join("real")).unwrap();
    fs::create_dir(workspace.path().join("named")).unwrap();
    fs::write(workspace.path().join("real/patch.audulus4"), REPRESENTATIVE).unwrap();
    symlink(
        Path::new("../real/patch.audulus4"),
        workspace.path().join("named/patch.audulus4"),
    )
    .unwrap();
    fs::write(workspace.path().join("named/angular_smoother.lua"), LUA).unwrap();

    let embed = run(
        &workspace,
        ["embed", "named/patch.audulus4", "angular_smoother.lua"],
    );

    assert_result(&embed, 0, "angular_smoother.lua\trecord\n", "");
}

#[cfg(target_os = "macos")]
fn document_with_live_blocker(bytes: &[u8]) -> Vec<u8> {
    let document = Document::parse(bytes).unwrap();
    let node = document
        .sources()
        .iter()
        .find(|node| node.kind == NodeKind::Dsp)
        .unwrap();
    let separator = if node.source.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    let expanded = format!(
        concat!(
            "{}{}",
            "-- @{{ alias/live-blocker.lua\n",
            "blocker witness\n",
            "-- @}} alias/live-blocker.lua\n",
            "-- @{{ real/live-blocker.lua\n",
            "other blocker source\n",
            "-- @}} real/live-blocker.lua\n",
        ),
        node.source, separator
    );
    document
        .rewrite(&[(node.handle, expanded.as_str())])
        .unwrap()
        .into_owned()
}

#[cfg(target_os = "macos")]
fn replace_document(path: &Path, bytes: &[u8]) {
    let replacement = path.with_file_name("patch.audulus4.replacement");
    fs::write(&replacement, bytes).unwrap();
    fs::rename(replacement, path).unwrap();
}

#[cfg(target_os = "macos")]
fn live_inventory_is_synchronized(workspace: &TempDir) -> bool {
    run(
        workspace,
        ["status", "--source-root", "source", "patch.audulus4"],
    )
    .status
    .code()
        == Some(0)
}

#[cfg(target_os = "macos")]
fn wait_for_live_consequence(
    live: &mut LiveChild,
    description: &str,
    mut observed: impl FnMut() -> bool,
) {
    let deadline = Instant::now() + LIVE_DEADLINE;
    loop {
        if let Some(status) = live.child_mut().try_wait().unwrap() {
            let output = live.take_output();
            panic!(
                "live exited before it could {description}: {status}\n{}",
                stderr(&output)
            );
        }
        if observed() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for live to {description}"
        );
        thread::sleep(LIVE_POLL_INTERVAL);
    }
}

#[cfg(target_os = "macos")]
fn wait_for_live_exit(live: &mut LiveChild, description: &str) {
    let deadline = Instant::now() + LIVE_DEADLINE;
    loop {
        if live.child_mut().try_wait().unwrap().is_some() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for live to {description}"
        );
        thread::sleep(LIVE_POLL_INTERVAL);
    }
}

#[cfg(target_os = "macos")]
fn assert_live_diagnostic_category(diagnostics: &str, category: &str) {
    assert!(
        diagnostics
            .lines()
            .any(|line| line.starts_with("zwirn live:") && line.contains(category)),
        "live emitted no `{category}` diagnostic category:\n{diagnostics}"
    );
}

#[cfg(target_os = "macos")]
struct LiveChild {
    child: Option<Child>,
}

#[cfg(target_os = "macos")]
impl LiveChild {
    fn new(child: Child) -> Self {
        Self { child: Some(child) }
    }

    fn child_mut(&mut self) -> &mut Child {
        self.child.as_mut().expect("live child is still owned")
    }

    fn id(&self) -> u32 {
        self.child.as_ref().expect("live child is still owned").id()
    }

    fn wait_with_output(mut self) -> Output {
        self.take_output()
    }

    fn take_output(&mut self) -> Output {
        self.child
            .take()
            .expect("live child is still owned")
            .wait_with_output()
            .unwrap()
    }
}

#[cfg(target_os = "macos")]
impl Drop for LiveChild {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            if child.try_wait().ok().flatten().is_none() {
                let _ = child.kill();
            }
            let _ = child.wait();
        }
    }
}

fn representative_workspace() -> TempDir {
    let workspace = tempfile::tempdir().unwrap();
    fs::write(workspace.path().join("patch.audulus4"), REPRESENTATIVE).unwrap();
    workspace
}

fn run<const N: usize>(workspace: &TempDir, args: [&str; N]) -> Output {
    command(workspace).args(args).output().unwrap()
}

fn command(workspace: &TempDir) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_zwirn"));
    command.current_dir(workspace.path());
    command
}

fn assert_result(output: &Output, code: i32, stdout: &str, expected_stderr: &str) {
    assert_eq!(output.status.code(), Some(code), "{}", stderr(output));
    assert_eq!(String::from_utf8_lossy(&output.stdout), stdout);
    assert_eq!(String::from_utf8_lossy(&output.stderr), expected_stderr);
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}
