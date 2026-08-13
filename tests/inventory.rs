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
fn regular_file_symlink_targets_follow_trusted_workspace_paths() {
    use std::os::unix::fs::symlink;

    let workspace = tempdir().unwrap();
    fs::write(workspace.path().join("real.lua"), b"source\n").unwrap();
    symlink("real.lua", workspace.path().join("linked.lua")).unwrap();
    let document =
        document_with_sources(["-- @{ linked.lua\nsource\n-- @} linked.lua\n", "", "", ""]);

    let inventory = Inventory::discover(document, workspace.path()).unwrap();
    assert_eq!(
        inventory.entries()[0].filesystem.as_ref().unwrap(),
        &CanonicalSource::try_from("source").unwrap()
    );
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
