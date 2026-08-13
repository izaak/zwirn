use std::fs;
use std::path::Path;

use tempfile::tempdir;

use zwirn::adls::Document;
use zwirn::engine::{Error, ExitState, Mode, Report, ReportEntry, Request, execute};
use zwirn::fragment::{BaselineHash, CanonicalSource, FragmentPath};
use zwirn::inventory::{Inventory, InventoryError};
use zwirn::reconcile::{Action, Operation, State};

const SOURCE_TYPES: &[u8] = include_bytes!("fixtures/source-types.audulus4");

#[test]
fn sync_applies_both_safe_directions_alongside_a_conflict() {
    let workspace = tempdir().unwrap();
    let source_root = workspace.path().join("sources");
    fs::create_dir(&source_root).unwrap();

    let baseline = source("baseline");
    let baseline_hash = BaselineHash::from_source(&baseline);
    let embedded_extract = source("embedded extract");
    let embedded_conflict = source("embedded conflict");
    let filesystem_embed = source("filesystem embed");
    let filesystem_conflict = source("filesystem conflict");

    // Node order deliberately differs from canonical fragment-path order.
    let dsp = format!(
        "-- @{{ c-conflict.lua\n{}-- @}} c-conflict.lua {baseline_hash}\n",
        embedded_conflict.as_str()
    );
    let lyte = format!(
        "// @{{ a-embed.lyte\n{}// @}} a-embed.lyte {baseline_hash}\n",
        baseline.as_str()
    );
    let canvas = format!(
        "-- @{{ b-extract.lua\n{}-- @}} b-extract.lua {baseline_hash}\n",
        embedded_extract.as_str()
    );
    let document_bytes = document_with_sources([&dsp, &lyte, &canvas, ""]);
    let document_path = workspace.path().join("patch.audulus4");
    fs::write(&document_path, document_bytes).unwrap();
    fs::write(source_root.join("a-embed.lyte"), b"filesystem embed\r\n").unwrap();
    fs::write(source_root.join("b-extract.lua"), baseline.as_str()).unwrap();
    fs::write(
        source_root.join("c-conflict.lua"),
        filesystem_conflict.as_str(),
    )
    .unwrap();

    let report = execute(Request {
        cwd: workspace.path(),
        document: Path::new("patch.audulus4"),
        source_root: Some(Path::new("sources")),
        selectors: &[],
        mode: Mode::Mutate(Operation::Sync),
    })
    .unwrap();

    assert_eq!(
        report,
        Report {
            entries: vec![
                ReportEntry::Action {
                    path: path("a-embed.lyte"),
                    action: Action::Embed,
                },
                ReportEntry::Action {
                    path: path("b-extract.lua"),
                    action: Action::Extract,
                },
                ReportEntry::State {
                    path: path("c-conflict.lua"),
                    state: State::Conflict,
                },
            ],
            exit: ExitState::Attention,
        }
    );
    assert_eq!(
        fs::read(source_root.join("a-embed.lyte")).unwrap(),
        b"filesystem embed\r\n"
    );
    assert_eq!(
        fs::read(source_root.join("b-extract.lua")).unwrap(),
        embedded_extract.as_str().as_bytes()
    );
    assert_eq!(
        fs::read(source_root.join("c-conflict.lua")).unwrap(),
        filesystem_conflict.as_str().as_bytes()
    );
    let final_inventory =
        Inventory::discover(fs::read(&document_path).unwrap(), &source_root).unwrap();
    let [embed, extract, conflict] = final_inventory.entries() else {
        panic!("expected exactly three fragments");
    };
    assert_eq!(embed.embedded, filesystem_embed);
    assert_eq!(
        embed.baseline,
        Some(BaselineHash::from_source(&filesystem_embed))
    );
    assert_eq!(extract.embedded, embedded_extract);
    assert_eq!(
        extract.baseline,
        Some(BaselineHash::from_source(&embedded_extract))
    );
    assert_eq!(conflict.embedded, embedded_conflict);
    assert_eq!(conflict.baseline, Some(baseline_hash));
}

#[test]
fn explicit_extract_recreates_missing_source_without_touching_the_document() {
    let workspace = tempdir().unwrap();
    let embedded = source("first line\r\nsecond line");
    let baseline = BaselineHash::from_source(&embedded);
    let dsp = format!(
        concat!(
            "-- @{{ nested/missing.lua\r\n",
            "first line\r\n",
            "second line\r\n",
            "-- @}} nested/missing.lua {baseline}\r\n"
        ),
        baseline = baseline
    );
    let document_bytes = document_with_sources([&dsp, "", "", ""]);
    let document_path = workspace.path().join("patch.audulus4");
    fs::write(&document_path, &document_bytes).unwrap();
    let mut permissions = fs::metadata(&document_path).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&document_path, permissions).unwrap();
    let selected = path("nested/missing.lua");

    let report = execute(Request {
        cwd: workspace.path(),
        document: Path::new("patch.audulus4"),
        source_root: None,
        selectors: std::slice::from_ref(&selected),
        mode: Mode::Mutate(Operation::Extract { force: false }),
    })
    .unwrap();

    assert_eq!(
        report,
        Report {
            entries: vec![ReportEntry::Action {
                path: selected,
                action: Action::Extract,
            }],
            exit: ExitState::Synchronized,
        }
    );
    assert_eq!(
        fs::read(workspace.path().join("nested/missing.lua")).unwrap(),
        embedded.as_str().as_bytes()
    );
    assert_eq!(fs::read(&document_path).unwrap(), document_bytes);
}

#[test]
fn invalid_prepared_markers_abort_before_any_output_is_written() {
    let workspace = tempdir().unwrap();
    let baseline = source("baseline");
    let baseline_hash = BaselineHash::from_source(&baseline);
    let embedded_extract = source("embedded change");
    let dsp = format!(
        concat!(
            "-- @{{ a-extract.lua\n{}-- @}} a-extract.lua {}\n",
            "-- @{{ b-embed.lua\n{}-- @}} b-embed.lua {}\n",
        ),
        embedded_extract.as_str(),
        baseline_hash,
        baseline.as_str(),
        baseline_hash,
    );
    let document_bytes = document_with_sources([&dsp, "", "", ""]);
    let document_path = workspace.path().join("patch.audulus4");
    fs::write(&document_path, &document_bytes).unwrap();
    fs::write(workspace.path().join("a-extract.lua"), baseline.as_str()).unwrap();
    let invalid_replacement = b"-- @{ nested.lua\nsource\n";
    fs::write(workspace.path().join("b-embed.lua"), invalid_replacement).unwrap();

    let error = execute(Request {
        cwd: workspace.path(),
        document: Path::new("patch.audulus4"),
        source_root: None,
        selectors: &[],
        mode: Mode::Mutate(Operation::Sync),
    })
    .unwrap_err();

    assert!(matches!(error, Error::InvalidPreparedMarkers { .. }));
    assert_eq!(
        fs::read(workspace.path().join("a-extract.lua")).unwrap(),
        baseline.as_str().as_bytes()
    );
    assert_eq!(
        fs::read(workspace.path().join("b-embed.lua")).unwrap(),
        invalid_replacement
    );
    assert_eq!(fs::read(document_path).unwrap(), document_bytes);
}

#[test]
fn rejects_a_fragment_target_lexically_equal_to_the_document() {
    let workspace = tempdir().unwrap();
    let document_bytes = document_with_sources([
        "-- @{ patch.audulus4\nsource\n-- @} patch.audulus4\n",
        "",
        "",
        "",
    ]);
    let document_path = workspace.path().join("patch.audulus4");
    fs::write(&document_path, &document_bytes).unwrap();

    let error = execute(Request {
        cwd: workspace.path(),
        document: Path::new("patch.audulus4"),
        source_root: None,
        selectors: &[],
        mode: Mode::Status,
    })
    .unwrap_err();

    assert!(matches!(
        error,
        Error::Inventory(InventoryError::DocumentTarget { path, target })
            if path.as_str() == "patch.audulus4" && target == document_path
    ));
    assert_eq!(fs::read(document_path).unwrap(), document_bytes);
}

fn source(value: &str) -> CanonicalSource {
    CanonicalSource::try_from(value).unwrap()
}

fn path(value: &str) -> FragmentPath {
    FragmentPath::try_from(value).unwrap()
}

fn document_with_sources(sources: [&str; 4]) -> Vec<u8> {
    let document = Document::parse(SOURCE_TYPES).unwrap();
    let replacements = document
        .sources()
        .iter()
        .zip(sources)
        .map(|(node, source)| (node.handle, source))
        .collect::<Vec<_>>();
    document.rewrite(&replacements).unwrap().into_owned()
}
