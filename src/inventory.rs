//! Owned fragment discovery from one read of each filesystem input.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::str::Utf8Error;

use thiserror::Error;

use crate::adls::Document;
use crate::fragment::{
    BaselineHash, CanonicalSource, CanonicalSourceError, FragmentPath, ParseError, ParsedSource,
};

/// A path-sorted, owned view of every fragment and filesystem input.
#[derive(Debug)]
pub struct Inventory {
    document_bytes: Vec<u8>,
    entries: Vec<InventoryEntry>,
}

impl Inventory {
    /// Discovers all fragments and validates all corresponding targets.
    pub fn discover(
        document_bytes: Vec<u8>,
        source_root: impl AsRef<Path>,
    ) -> Result<Self, InventoryError> {
        let source_root = source_root.as_ref().to_path_buf();
        validate_source_root(&source_root).map_err(|source| InventoryError::SourceRoot {
            path: source_root.clone(),
            source,
        })?;
        let document = Document::parse(&document_bytes)
            .map_err(|source| InventoryError::InvalidDocument { source })?;

        let mut discovered = Vec::new();
        for node in document.sources() {
            let parsed = ParsedSource::parse(node.kind, node.source).map_err(|source| {
                InventoryError::InvalidMarkers {
                    node_index: node.handle.index(),
                    source,
                }
            })?;
            for fragment in parsed.fragments() {
                let embedded = CanonicalSource::try_from(fragment.source).map_err(|source| {
                    InventoryError::InvalidEmbeddedSource {
                        node_index: node.handle.index(),
                        path: fragment.path.clone(),
                        source,
                    }
                })?;
                discovered.push(DiscoveredFragment {
                    node_index: node.handle.index(),
                    path: fragment.path.clone(),
                    embedded,
                    baseline: fragment.baseline,
                });
            }
        }
        drop(document);

        discovered.sort_by(|left, right| left.path.cmp(&right.path));
        validate_uniqueness(&discovered)?;

        let mut entries = Vec::with_capacity(discovered.len());
        for fragment in discovered {
            validate_parents(&source_root, &fragment.path)?;
            let target = target_path(&source_root, &fragment.path);
            let target_bytes = read_optional_target(&fragment.path, &target)?;
            let filesystem = match target_bytes.as_deref() {
                None => None,
                Some(bytes) => {
                    let text = std::str::from_utf8(bytes).map_err(|source| {
                        InventoryError::InvalidTargetUtf8 {
                            path: fragment.path.clone(),
                            target: target.clone(),
                            source,
                        }
                    })?;
                    Some(CanonicalSource::try_from(text).map_err(|source| {
                        InventoryError::InvalidTargetSource {
                            path: fragment.path.clone(),
                            target: target.clone(),
                            source,
                        }
                    })?)
                }
            };
            entries.push(InventoryEntry {
                node_index: fragment.node_index,
                path: fragment.path,
                embedded: fragment.embedded,
                baseline: fragment.baseline,
                target,
                filesystem,
            });
        }

        Ok(Self {
            document_bytes,
            entries,
        })
    }

    pub fn document_bytes(&self) -> &[u8] {
        &self.document_bytes
    }

    pub fn entries(&self) -> &[InventoryEntry] {
        &self.entries
    }

    /// Selects an exact, deduplicated subset in canonical path order.
    /// An empty selector list selects the complete inventory.
    pub fn select(
        &self,
        selectors: &[FragmentPath],
    ) -> Result<Vec<&InventoryEntry>, SelectorError> {
        if selectors.is_empty() {
            return Ok(self.entries.iter().collect());
        }

        let selectors = selectors.iter().collect::<BTreeSet<_>>();
        let mut selected = Vec::with_capacity(selectors.len());
        for selector in selectors {
            let index = self
                .entries
                .binary_search_by(|entry| entry.path.cmp(selector))
                .map_err(|_| SelectorError::UnknownPath {
                    path: selector.clone(),
                })?;
            selected.push(&self.entries[index]);
        }
        Ok(selected)
    }
}

/// One fragment and its validated filesystem input.
#[derive(Debug)]
pub struct InventoryEntry {
    pub node_index: u32,
    pub path: FragmentPath,
    pub embedded: CanonicalSource,
    pub baseline: Option<BaselineHash>,
    pub target: PathBuf,
    pub filesystem: Option<CanonicalSource>,
}

#[derive(Debug)]
struct DiscoveredFragment {
    node_index: u32,
    path: FragmentPath,
    embedded: CanonicalSource,
    baseline: Option<BaselineHash>,
}

fn validate_uniqueness(discovered: &[DiscoveredFragment]) -> Result<(), InventoryError> {
    for pair in discovered.windows(2) {
        if pair[0].path == pair[1].path {
            return Err(InventoryError::DuplicateFragment {
                path: pair[0].path.clone(),
                first_node: pair[0].node_index,
                second_node: pair[1].node_index,
            });
        }
    }
    Ok(())
}

fn target_path(source_root: &Path, path: &FragmentPath) -> PathBuf {
    let mut target = source_root.to_path_buf();
    for segment in path.as_str().split('/') {
        target.push(segment);
    }
    target
}

fn validate_parents(source_root: &Path, path: &FragmentPath) -> Result<(), InventoryError> {
    let mut parent = source_root.to_path_buf();
    let mut segments = path.as_str().split('/').peekable();
    while let Some(segment) = segments.next() {
        if segments.peek().is_none() {
            break;
        }
        parent.push(segment);
        validate_optional_directory(&parent).map_err(|source| InventoryError::FragmentParent {
            path: path.clone(),
            parent: parent.clone(),
            source,
        })?;
    }
    Ok(())
}

fn validate_source_root(path: &Path) -> Result<(), io::Error> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            "path is not a directory",
        ));
    }
    Ok(())
}

fn validate_optional_directory(path: &Path) -> Result<(), io::Error> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            "path is not a directory",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn read_optional_target(
    path: &FragmentPath,
    target: &Path,
) -> Result<Option<Vec<u8>>, InventoryError> {
    let metadata = match fs::metadata(target) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(InventoryError::TargetAccess {
                path: path.clone(),
                target: target.to_path_buf(),
                source,
            });
        }
    };
    if !metadata.is_file() {
        return Err(InventoryError::TargetNotRegular {
            path: path.clone(),
            target: target.to_path_buf(),
        });
    }
    fs::read(target)
        .map(Some)
        .map_err(|source| InventoryError::TargetRead {
            path: path.clone(),
            target: target.to_path_buf(),
            source,
        })
}

/// A discovery or filesystem-validation failure.
#[derive(Debug, Error)]
pub enum InventoryError {
    #[error("source root `{}` is not an accessible directory: {source}", path.display())]
    SourceRoot {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("invalid Audulus document: {source}")]
    InvalidDocument {
        #[source]
        source: crate::adls::Error,
    },

    #[error("invalid fragment markers in source node {node_index}: {source}")]
    InvalidMarkers {
        node_index: u32,
        #[source]
        source: ParseError,
    },

    #[error("invalid embedded source for fragment `{path}` in node {node_index}: {source}")]
    InvalidEmbeddedSource {
        node_index: u32,
        path: FragmentPath,
        #[source]
        source: CanonicalSourceError,
    },

    #[error("fragment `{path}` occurs in both source nodes {first_node} and {second_node}")]
    DuplicateFragment {
        path: FragmentPath,
        first_node: u32,
        second_node: u32,
    },

    #[error("cannot validate parent `{}` for fragment `{path}`: {source}", parent.display())]
    FragmentParent {
        path: FragmentPath,
        parent: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("cannot access fragment `{path}` target `{}`: {source}", target.display())]
    TargetAccess {
        path: FragmentPath,
        target: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("fragment `{path}` target `{}` is not a regular file", target.display())]
    TargetNotRegular { path: FragmentPath, target: PathBuf },

    #[error("cannot read fragment `{path}` target `{}`: {source}", target.display())]
    TargetRead {
        path: FragmentPath,
        target: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("fragment `{path}` target `{}` is not valid UTF-8: {source}", target.display())]
    InvalidTargetUtf8 {
        path: FragmentPath,
        target: PathBuf,
        #[source]
        source: Utf8Error,
    },

    #[error("fragment `{path}` target `{}` is not canonicalizable source: {source}", target.display())]
    InvalidTargetSource {
        path: FragmentPath,
        target: PathBuf,
        #[source]
        source: CanonicalSourceError,
    },
}

/// A selector that does not match the discovered inventory.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum SelectorError {
    #[error("fragment `{path}` is not present in the document")]
    UnknownPath { path: FragmentPath },
}
