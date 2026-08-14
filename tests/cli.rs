use std::fs;
#[cfg(unix)]
use std::path::Path;
use std::process::{Command, Output, Stdio};

use tempfile::TempDir;

const REPRESENTATIVE: &[u8] = include_bytes!("fixtures/representative.audulus4");
const EMPTY: &[u8] = include_bytes!("fixtures/empty.audulus4");
const LUA: &[u8] = include_bytes!("fixtures/angular_smoother.lua");
const LYTE: &[u8] = include_bytes!("fixtures/angular_smoother.lyte");

#[test]
fn root_help_explains_shared_operands() {
    let workspace = tempfile::tempdir().unwrap();
    let help = run(&workspace, ["--help"]);

    assert_eq!(help.status.code(), Some(0));
    assert!(help.stderr.is_empty());

    let stdout = String::from_utf8_lossy(&help.stdout);
    assert!(stdout.contains("Usage: zwirn <COMMAND> [OPTIONS] <DOCUMENT> [FRAGMENT]..."));
    assert!(stdout.contains("DOCUMENT is an .audulus4 file."));
    assert!(stdout.contains("FRAGMENT is an exact marker path relative to the source root."));
    assert!(stdout.contains("zwirn status patch.audulus4"));
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
