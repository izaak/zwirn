//! Owned fragment discovery from one read of each filesystem input.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{self, Read};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::str::Utf8Error;

use cap_std::fs::MetadataExt as CapMetadataExt;
use thiserror::Error;

use crate::access::{AccessPolicy, DirectAccess, PolicyFailure, direct_result, flatten};
use crate::adls::Document;
use crate::fragment::{
    BaselineHash, CanonicalSource, CanonicalSourceError, FragmentPath, ParseError, ParsedSource,
};
use crate::source_root::{FileIdentity, SourceRoot, relative_path};

/// A path-sorted, owned view of every fragment and filesystem input.
#[derive(Debug)]
pub struct Inventory {
    source_root: SourceRoot,
    document_bytes: Vec<u8>,
    entries: Vec<InventoryEntry>,
}

impl Inventory {
    /// Discovers all fragments and validates all corresponding targets.
    pub fn discover(
        document_bytes: Vec<u8>,
        source_root: impl AsRef<Path>,
    ) -> Result<Self, InventoryError> {
        let mut access = DirectAccess;
        direct_result(Self::discover_with_access(
            document_bytes,
            source_root.as_ref(),
            &mut access,
        ))
    }

    pub(crate) fn discover_for_document_with_access<P: AccessPolicy>(
        source_root: &Path,
        document_path: &Path,
        access: &mut P,
    ) -> Result<Self, PolicyFailure<P::Error, InventoryError>> {
        let source_root = open_source_root(source_root).map_err(PolicyFailure::Operation)?;
        let document = flatten(access.read(document_path, || read_document(document_path)))?;
        let origin = DocumentOrigin {
            path: document_path,
            identity: document.identity,
        };
        Self::discover_inner(document.bytes, source_root, Some(origin), access)
    }

    pub(crate) fn discover_with_access<P: AccessPolicy>(
        document_bytes: Vec<u8>,
        source_root: &Path,
        access: &mut P,
    ) -> Result<Self, PolicyFailure<P::Error, InventoryError>> {
        let source_root = open_source_root(source_root).map_err(PolicyFailure::Operation)?;
        Self::discover_inner(document_bytes, source_root, None, access)
    }

    fn discover_inner<P: AccessPolicy>(
        document_bytes: Vec<u8>,
        source_root: SourceRoot,
        document_origin: Option<DocumentOrigin<'_>>,
        access: &mut P,
    ) -> Result<Self, PolicyFailure<P::Error, InventoryError>> {
        let document = Document::parse(&document_bytes).map_err(|source| {
            PolicyFailure::Operation(InventoryError::InvalidDocument { source })
        })?;

        let mut discovered = Vec::new();
        for node in document.sources() {
            let parsed = ParsedSource::parse(node.kind, node.source).map_err(|source| {
                PolicyFailure::Operation(InventoryError::InvalidMarkers {
                    node_index: node.handle.index(),
                    source,
                })
            })?;
            for fragment in parsed.fragments() {
                let embedded = CanonicalSource::try_from(fragment.source).map_err(|source| {
                    PolicyFailure::Operation(InventoryError::InvalidEmbeddedSource {
                        node_index: node.handle.index(),
                        path: fragment.path.clone(),
                        source,
                    })
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
        validate_uniqueness(&discovered).map_err(PolicyFailure::Operation)?;
        validate_path_prefixes(&discovered).map_err(PolicyFailure::Operation)?;

        let mut entries = Vec::with_capacity(discovered.len());
        let mut target_identities = BTreeMap::<FileIdentity, FragmentPath>::new();
        for fragment in discovered {
            validate_parents(&source_root, &fragment.path).map_err(PolicyFailure::Operation)?;
            let target = source_root.named_path(relative_path(&fragment.path));
            if document_origin.is_some_and(|document| document.path == target) {
                return Err(PolicyFailure::Operation(InventoryError::DocumentTarget {
                    path: fragment.path,
                    target,
                }));
            }
            let target_input = flatten(access.read(&target, || {
                read_optional_target(&source_root, &fragment.path, &target)
            }))?;
            let filesystem = match target_input {
                None => None,
                Some(input) => {
                    if document_origin.is_some_and(|document| document.identity == input.identity) {
                        return Err(PolicyFailure::Operation(InventoryError::DocumentTarget {
                            path: fragment.path,
                            target,
                        }));
                    }
                    if let Some(first_path) = target_identities.get(&input.identity) {
                        return Err(PolicyFailure::Operation(InventoryError::AliasedTargets {
                            first_path: first_path.clone(),
                            second_path: fragment.path,
                        }));
                    }
                    target_identities.insert(input.identity, fragment.path.clone());

                    let text = std::str::from_utf8(&input.bytes).map_err(|source| {
                        PolicyFailure::Operation(InventoryError::InvalidTargetUtf8 {
                            path: fragment.path.clone(),
                            target: target.clone(),
                            source,
                        })
                    })?;
                    Some(CanonicalSource::try_from(text).map_err(|source| {
                        PolicyFailure::Operation(InventoryError::InvalidTargetSource {
                            path: fragment.path.clone(),
                            target: target.clone(),
                            source,
                        })
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
            source_root,
            document_bytes,
            entries,
        })
    }

    pub fn document_bytes(&self) -> &[u8] {
        &self.document_bytes
    }

    pub(crate) fn source_root(&self) -> &SourceRoot {
        &self.source_root
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

#[derive(Clone, Copy)]
struct DocumentOrigin<'a> {
    path: &'a Path,
    identity: FileIdentity,
}

struct ExistingInput {
    bytes: Vec<u8>,
    identity: FileIdentity,
}

fn open_source_root(path: &Path) -> Result<SourceRoot, InventoryError> {
    SourceRoot::open(path).map_err(|source| InventoryError::SourceRoot {
        path: path.to_path_buf(),
        source,
    })
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

fn validate_path_prefixes(discovered: &[DiscoveredFragment]) -> Result<(), InventoryError> {
    for fragment in discovered {
        for (separator, _) in fragment.path.as_str().match_indices('/') {
            let ancestor = &fragment.path.as_str()[..separator];
            if let Ok(index) =
                discovered.binary_search_by(|candidate| candidate.path.as_str().cmp(ancestor))
            {
                return Err(InventoryError::PathPrefixConflict {
                    ancestor: discovered[index].path.clone(),
                    descendant: fragment.path.clone(),
                });
            }
        }
    }
    Ok(())
}

fn validate_parents(source_root: &SourceRoot, path: &FragmentPath) -> Result<(), InventoryError> {
    let mut parent = PathBuf::new();
    if let Some(segments) = relative_path(path).parent() {
        for segment in segments {
            parent.push(segment);
            validate_optional_directory(source_root, &parent).map_err(|source| {
                InventoryError::FragmentParent {
                    path: path.clone(),
                    parent: source_root.named_path(&parent),
                    source,
                }
            })?;
        }
    }
    Ok(())
}

fn validate_optional_directory(source_root: &SourceRoot, path: &Path) -> Result<(), io::Error> {
    match source_root.metadata(path) {
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
    source_root: &SourceRoot,
    path: &FragmentPath,
    target: &Path,
) -> Result<Option<ExistingInput>, InventoryError> {
    let metadata = match source_root.metadata(relative_path(path)) {
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

    let mut file = match source_root.open_target(path) {
        Ok(file) => file,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(InventoryError::TargetAccess {
                path: path.clone(),
                target: target.to_path_buf(),
                source,
            });
        }
    };
    let metadata = file
        .metadata()
        .map_err(|source| InventoryError::TargetAccess {
            path: path.clone(),
            target: target.to_path_buf(),
            source,
        })?;
    if !metadata.is_file() {
        return Err(InventoryError::TargetNotRegular {
            path: path.clone(),
            target: target.to_path_buf(),
        });
    }
    let identity = FileIdentity::new(metadata.dev(), metadata.ino());
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|source| InventoryError::TargetRead {
            path: path.clone(),
            target: target.to_path_buf(),
            source,
        })?;
    Ok(Some(ExistingInput { bytes, identity }))
}

fn read_document(path: &Path) -> Result<ExistingInput, InventoryError> {
    let mut file = File::open(path).map_err(|source| InventoryError::DocumentInput {
        path: path.to_path_buf(),
        source,
    })?;
    let metadata = file
        .metadata()
        .map_err(|source| InventoryError::DocumentInput {
            path: path.to_path_buf(),
            source,
        })?;
    let identity = FileIdentity::new(metadata.dev(), metadata.ino());
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|source| InventoryError::DocumentInput {
            path: path.to_path_buf(),
            source,
        })?;
    Ok(ExistingInput { bytes, identity })
}

/// A discovery or filesystem-validation failure.
#[derive(Debug, Error)]
pub enum InventoryError {
    #[error("cannot read Audulus document `{}`: {source}", path.display())]
    DocumentInput {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

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

    #[error("fragment path `{ancestor}` is a strict component ancestor of `{descendant}`")]
    PathPrefixConflict {
        ancestor: FragmentPath,
        descendant: FragmentPath,
    },

    #[error("fragment `{path}` targets the Audulus document `{}`", target.display())]
    DocumentTarget { path: FragmentPath, target: PathBuf },

    #[error("fragments `{first_path}` and `{second_path}` target the same existing file")]
    AliasedTargets {
        first_path: FragmentPath,
        second_path: FragmentPath,
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
