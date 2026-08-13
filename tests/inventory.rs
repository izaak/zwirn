use std::fs;

use tempfile::tempdir;

use zwirn::adls::Document;
use zwirn::fragment::{CanonicalSource, FragmentPath};
use zwirn::inventory::{Inventory, InventoryError, SelectorError};

const SOURCE_TYPES: &[u8] = include_bytes!("fixtures/source-types.audulus4");

#[test]
fn discovers_owned_fragments_in_path_order_and_selects_exact_paths() {
    let workspace = tempdir().unwrap();
    fs::write(workspace.path().join("a.lyte"), b"embedded a\r\n").unwrap();
    let document = document_with_sources([
        "-- @{ z.lua\r\nembedded z\r\n-- @} z.lua\r\n",
        "// @{ a.lyte\nembedded a\n// @} a.lyte\n",
        "",
        "",
    ]);

    let inventory = Inventory::discover(document, workspace.path()).unwrap();
    assert_eq!(
        inventory
            .entries()
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>(),
        ["a.lyte", "z.lua"]
    );
    assert_eq!(
        inventory.entries()[0].embedded,
        CanonicalSource::try_from("embedded a").unwrap()
    );
    assert_eq!(
        inventory.entries()[0].filesystem.as_ref().unwrap(),
        &CanonicalSource::try_from("embedded a").unwrap()
    );
    assert!(inventory.entries()[1].filesystem.is_none());

    let z = FragmentPath::try_from("z.lua").unwrap();
    let a = FragmentPath::try_from("a.lyte").unwrap();
    let selected = inventory.select(&[z.clone(), a, z]).unwrap();
    assert_eq!(
        selected
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>(),
        ["a.lyte", "z.lua"]
    );
    assert_eq!(inventory.select(&[]).unwrap().len(), 2);

    let unknown = FragmentPath::try_from("unknown.lua").unwrap();
    assert!(matches!(
        inventory.select(&[unknown]),
        Err(SelectorError::UnknownPath { path }) if path.as_str() == "unknown.lua"
    ));
}

#[test]
fn rejects_global_duplicates() {
    let workspace = tempdir().unwrap();
    let duplicate = document_with_sources([
        "-- @{ same.code\none\n-- @} same.code\n",
        "// @{ same.code\ntwo\n// @} same.code\n",
        "",
        "",
    ]);
    assert!(matches!(
        Inventory::discover(duplicate, workspace.path()),
        Err(InventoryError::DuplicateFragment { path, .. }) if path.as_str() == "same.code"
    ));
}

#[test]
fn validates_every_existing_target_before_selection() {
    let workspace = tempdir().unwrap();
    fs::write(workspace.path().join("selected.lua"), b"selected\n").unwrap();
    fs::write(workspace.path().join("invalid.lua"), [0xff]).unwrap();
    let document = document_with_sources([
        concat!(
            "-- @{ selected.lua\nselected\n-- @} selected.lua\n",
            "-- @{ invalid.lua\nembedded\n-- @} invalid.lua\n",
        ),
        "",
        "",
        "",
    ]);

    assert!(matches!(
        Inventory::discover(document, workspace.path()),
        Err(InventoryError::InvalidTargetUtf8 { path, .. })
            if path.as_str() == "invalid.lua"
    ));

    let directory_workspace = tempdir().unwrap();
    fs::create_dir(directory_workspace.path().join("target.lua")).unwrap();
    let document =
        document_with_sources(["-- @{ target.lua\nembedded\n-- @} target.lua\n", "", "", ""]);
    assert!(matches!(
        Inventory::discover(document, directory_workspace.path()),
        Err(InventoryError::TargetNotRegular { path, .. })
            if path.as_str() == "target.lua"
    ));
}

#[cfg(unix)]
#[test]
fn fifo_target_is_rejected_before_opening_can_block() {
    use std::process::Command;
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, Instant};

    let workspace = tempdir().unwrap();
    let fifo = workspace.path().join("target.lua");
    assert!(
        Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    let document =
        document_with_sources(["-- @{ target.lua\nembedded\n-- @} target.lua\n", "", "", ""]);

    // Release a blocking FIFO open after the deadline so a regression fails
    // promptly instead of hanging the test process.
    let (cancel_release, release_cancelled) = mpsc::channel();
    let release_fifo = fifo.clone();
    let release = thread::spawn(move || {
        if release_cancelled
            .recv_timeout(Duration::from_secs(2))
            .is_err()
        {
            fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(release_fifo)
                .unwrap();
        }
    });

    let started = Instant::now();
    let result = Inventory::discover(document, workspace.path());
    let elapsed = started.elapsed();
    let _ = cancel_release.send(());
    release.join().unwrap();

    assert!(matches!(
        result,
        Err(InventoryError::TargetNotRegular { path, .. })
            if path.as_str() == "target.lua"
    ));
    assert!(
        elapsed < Duration::from_secs(1),
        "FIFO validation blocked for {elapsed:?}"
    );
}

#[test]
fn source_root_must_exist_and_be_a_directory() {
    let workspace = tempdir().unwrap();
    let document = document_with_sources(["", "", "", ""]);
    let missing = workspace.path().join("missing");
    assert!(matches!(
        Inventory::discover(document.clone(), &missing),
        Err(InventoryError::SourceRoot { path, .. }) if path == missing
    ));

    let file = workspace.path().join("file");
    fs::write(&file, b"not a directory").unwrap();
    assert!(matches!(
        Inventory::discover(document, &file),
        Err(InventoryError::SourceRoot { path, .. }) if path == file
    ));
}

#[cfg(unix)]
#[test]
fn in_root_relative_directory_symlinks_are_followed() {
    use std::os::unix::fs::symlink;

    let workspace = tempdir().unwrap();
    fs::create_dir_all(workspace.path().join("aliases")).unwrap();
    fs::create_dir_all(workspace.path().join("real")).unwrap();
    fs::write(workspace.path().join("real/source.lua"), b"source\n").unwrap();
    symlink("../real", workspace.path().join("aliases/linked")).unwrap();
    let document = document_with_sources([
        "-- @{ aliases/linked/source.lua\nsource\n-- @} aliases/linked/source.lua\n",
        "",
        "",
        "",
    ]);

    let inventory = Inventory::discover(document, workspace.path()).unwrap();
    assert_eq!(
        inventory.entries()[0].filesystem.as_ref().unwrap(),
        &CanonicalSource::try_from("source").unwrap()
    );
}

#[cfg(unix)]
#[test]
fn absolute_symlinks_are_rejected_even_when_they_point_into_the_source_root() {
    use std::os::unix::fs::symlink;

    let workspace = tempdir().unwrap();
    let real = workspace.path().join("real.lua");
    fs::write(&real, b"source\n").unwrap();
    symlink(&real, workspace.path().join("absolute.lua")).unwrap();
    let document = document_with_sources([
        "-- @{ absolute.lua\nsource\n-- @} absolute.lua\n",
        "",
        "",
        "",
    ]);

    assert!(matches!(
        Inventory::discover(document, workspace.path()),
        Err(InventoryError::TargetAccess { path, .. }) if path.as_str() == "absolute.lua"
    ));
}

#[cfg(unix)]
#[test]
fn distinct_fragment_paths_cannot_alias_the_same_existing_file() {
    use std::os::unix::fs::symlink;

    let workspace = tempdir().unwrap();
    fs::write(workspace.path().join("real.lua"), b"source\n").unwrap();
    symlink("real.lua", workspace.path().join("linked.lua")).unwrap();
    let document = document_with_sources([
        concat!(
            "-- @{ linked.lua\nsource\n-- @} linked.lua\n",
            "-- @{ real.lua\nsource\n-- @} real.lua\n",
        ),
        "",
        "",
        "",
    ]);

    assert!(matches!(
        Inventory::discover(document, workspace.path()),
        Err(InventoryError::AliasedTargets {
            first_path,
            second_path,
        }) if first_path.as_str() == "linked.lua" && second_path.as_str() == "real.lua"
    ));
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
