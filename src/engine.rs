//! Synchronization orchestration from inventory through ordered commit.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::adls::{Document, NodeHandle};
use crate::commit::{self, DocumentOutput, ExternalOutput, ExternalWrite};
use crate::fragment::{
    FragmentPath, FragmentUpdate, ParsedSource, RewriteError as FragmentRewriteError,
};
use crate::inventory::{Inventory, InventoryEntry};
use crate::reconcile::{
    Action, Classification, Decision, Operation, PlanError, SelectionKind, State, classify, plan,
};

/// A read-only inspection or a mutating synchronization operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mode {
    Status,
    Mutate(Operation),
}

/// One resolved library request. Relative paths are interpreted from `cwd`.
#[derive(Clone, Copy, Debug)]
pub struct Request<'a> {
    pub cwd: &'a Path,
    pub document: &'a Path,
    pub source_root: Option<&'a Path>,
    pub selectors: &'a [FragmentPath],
    pub mode: Mode,
}

/// A normally completed command result.
#[derive(Debug, Eq, PartialEq)]
pub struct Report {
    pub entries: Vec<ReportEntry>,
    pub exit: ExitState,
}

/// One path-sorted observable command result.
#[derive(Debug, Eq, PartialEq)]
pub enum ReportEntry {
    State { path: FragmentPath, state: State },
    Action { path: FragmentPath, action: Action },
}

impl ReportEntry {
    pub fn path(&self) -> &FragmentPath {
        match self {
            Self::State { path, .. } | Self::Action { path, .. } => path,
        }
    }

    pub const fn result(&self) -> &dyn std::fmt::Display {
        match self {
            Self::State { state, .. } => state,
            Self::Action { action, .. } => action,
        }
    }
}

/// Whether selected fragments need further attention after normal completion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExitState {
    Synchronized,
    Attention,
}

impl ExitState {
    pub const fn code(self) -> u8 {
        match self {
            Self::Synchronized => 0,
            Self::Attention => 1,
        }
    }
}

/// Executes one complete Zwirn request.
pub fn execute(request: Request<'_>) -> Result<Report, Error> {
    let paths = resolve_paths(request.cwd, request.document, request.source_root)?;
    let inventory = Inventory::discover_for_document(&paths.source_root, &paths.document)?;

    let selection = if request.selectors.is_empty() {
        SelectionKind::All
    } else {
        SelectionKind::Explicit
    };
    let selected = inventory.select(request.selectors)?;
    let classified = selected
        .into_iter()
        .map(|entry| {
            let classification =
                classify(&entry.embedded, entry.baseline, entry.filesystem.as_ref());
            (entry, classification)
        })
        .collect::<Vec<_>>();

    match request.mode {
        Mode::Status => Ok(status_report(&classified)),
        Mode::Mutate(operation) => mutate(
            &paths.document,
            &inventory,
            &classified,
            selection,
            operation,
        ),
    }
}

struct ResolvedPaths {
    document: PathBuf,
    source_root: PathBuf,
}

fn resolve_paths(
    cwd: &Path,
    document: &Path,
    source_root: Option<&Path>,
) -> Result<ResolvedPaths, Error> {
    if document.extension() != Some(OsStr::new("audulus4")) {
        return Err(Error::InvalidDocumentExtension {
            path: document.to_path_buf(),
        });
    }
    let document = resolve_from(cwd, document);
    let source_root = match source_root {
        Some(source_root) => resolve_from(cwd, source_root),
        None => document
            .parent()
            .expect("a cwd-resolved document path has a parent")
            .to_path_buf(),
    };
    Ok(ResolvedPaths {
        document,
        source_root,
    })
}

fn resolve_from(cwd: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

fn status_report(classified: &[(&InventoryEntry, Classification)]) -> Report {
    let attention = classified
        .iter()
        .any(|(_, classification)| classification.state() != State::Synchronized);
    let entries = classified
        .iter()
        .map(|(entry, classification)| ReportEntry::State {
            path: entry.path.clone(),
            state: classification.state(),
        })
        .collect();
    Report {
        entries,
        exit: exit_state(attention),
    }
}

fn mutate(
    document_path: &Path,
    inventory: &Inventory,
    classified: &[(&InventoryEntry, Classification)],
    selection: SelectionKind,
    operation: Operation,
) -> Result<Report, Error> {
    let classifications = classified
        .iter()
        .map(|(_, classification)| *classification)
        .collect::<Vec<_>>();
    let decisions = plan(operation, selection, &classifications)
        .map_err(|source| map_plan_error(source, classified))?;

    let mut node_updates = BTreeMap::<u32, Vec<FragmentUpdate<'_>>>::new();
    let mut extracted = Vec::<&InventoryEntry>::new();
    let mut report_entries = Vec::new();
    let mut attention = false;

    for ((entry, _), decision) in classified.iter().zip(&decisions) {
        match *decision {
            Decision::Settled => {}
            Decision::Unresolved(state) => {
                attention = true;
                report_entries.push(ReportEntry::State {
                    path: entry.path.clone(),
                    state,
                });
            }
            Decision::Act(action) => {
                let update = match action {
                    Action::Record | Action::Extract => {
                        FragmentUpdate::Record { path: &entry.path }
                    }
                    Action::Embed => FragmentUpdate::Replace {
                        path: &entry.path,
                        source: entry
                            .filesystem
                            .as_ref()
                            .expect("an embed action has filesystem source"),
                    },
                };
                node_updates
                    .entry(entry.node_index)
                    .or_default()
                    .push(update);
                if action == Action::Extract {
                    extracted.push(entry);
                }
                report_entries.push(ReportEntry::Action {
                    path: entry.path.clone(),
                    action,
                });
            }
        }
    }

    let document_output = materialize_document(inventory.document_bytes(), node_updates)?;
    if extracted.is_empty() && document_output.is_none() {
        return Ok(Report {
            entries: report_entries,
            exit: exit_state(attention),
        });
    }

    let external = extracted
        .iter()
        .map(|entry| ExternalOutput {
            path: &entry.path,
            destination: &entry.target,
            bytes: entry.embedded.as_str().as_bytes(),
            write: if entry.filesystem.is_none() {
                ExternalWrite::CreateNew
            } else {
                ExternalWrite::CreateOrTruncate
            },
        })
        .collect::<Vec<_>>();
    let document = document_output.as_deref().map(|bytes| DocumentOutput {
        destination: document_path,
        bytes,
    });
    commit::commit(&external, document)?;

    Ok(Report {
        entries: report_entries,
        exit: exit_state(attention),
    })
}

fn materialize_document(
    document_bytes: &[u8],
    node_updates: BTreeMap<u32, Vec<FragmentUpdate<'_>>>,
) -> Result<Option<Vec<u8>>, Error> {
    let document = Document::parse(document_bytes)?;
    let mut rewritten_nodes = Vec::<(NodeHandle, String)>::new();
    for (node_index, updates) in node_updates {
        let node = document
            .sources()
            .binary_search_by_key(&node_index, |node| node.handle.index())
            .ok()
            .and_then(|index| document.sources().get(index))
            .ok_or(Error::InconsistentMaterializationNode { node_index })?;
        let parsed = ParsedSource::parse(node.kind, node.source)
            .map_err(|source| Error::InconsistentMaterializationMarkers { node_index, source })?;
        if let Cow::Owned(rewritten) = parsed
            .rewrite(&updates)
            .map_err(|source| Error::FragmentRewrite { node_index, source })?
        {
            rewritten_nodes.push((node.handle, rewritten));
        }
    }

    let replacements = rewritten_nodes
        .iter()
        .map(|(handle, source)| (*handle, source.as_str()))
        .collect::<Vec<_>>();
    match document.rewrite(&replacements)? {
        Cow::Borrowed(_) => Ok(None),
        Cow::Owned(bytes) => {
            validate_prepared_document(&bytes)?;
            Ok(Some(bytes))
        }
    }
}

fn validate_prepared_document(bytes: &[u8]) -> Result<(), Error> {
    let document = Document::parse(bytes)?;
    let mut paths = BTreeSet::new();
    for node in document.sources() {
        let parsed = ParsedSource::parse(node.kind, node.source).map_err(|source| {
            Error::InvalidPreparedMarkers {
                node_index: node.handle.index(),
                source,
            }
        })?;
        for fragment in parsed.fragments() {
            if !paths.insert(fragment.path.clone()) {
                return Err(Error::DuplicatePreparedFragment {
                    path: fragment.path.clone(),
                });
            }
        }
    }
    Ok(())
}

const fn exit_state(attention: bool) -> ExitState {
    if attention {
        ExitState::Attention
    } else {
        ExitState::Synchronized
    }
}

fn map_plan_error(error: PlanError, classified: &[(&InventoryEntry, Classification)]) -> Error {
    match error {
        PlanError::ForceRequiresExplicitSelection => Error::ForceRequiresExplicitSelection,
        PlanError::InvalidForcedState { index, state } => Error::InvalidForcedState {
            path: classified[index].0.path.clone(),
            state,
        },
        PlanError::AllocationFailed => Error::Allocation {
            operation: "plan synchronization actions",
        },
    }
}

/// A validation, materialization, or operational failure.
#[derive(Debug, Error)]
pub enum Error {
    #[error("document path `{}` does not have the `.audulus4` extension", path.display())]
    InvalidDocumentExtension { path: PathBuf },

    #[error(transparent)]
    Inventory(#[from] crate::inventory::InventoryError),

    #[error(transparent)]
    Selector(#[from] crate::inventory::SelectorError),

    #[error("--force requires at least one explicitly selected fragment")]
    ForceRequiresExplicitSelection,

    #[error("fragment `{path}` is in `{state}` state, which cannot be forced")]
    InvalidForcedState { path: FragmentPath, state: State },

    #[error("fragment inventory references unknown source node {node_index} for materialization")]
    InconsistentMaterializationNode { node_index: u32 },

    #[error("source node {node_index} has invalid fragment markers for materialization: {source}")]
    InconsistentMaterializationMarkers {
        node_index: u32,
        #[source]
        source: crate::fragment::ParseError,
    },

    #[error("cannot rewrite fragments in source node {node_index}: {source}")]
    FragmentRewrite {
        node_index: u32,
        #[source]
        source: FragmentRewriteError,
    },

    #[error("prepared source node {node_index} has invalid fragment markers: {source}")]
    InvalidPreparedMarkers {
        node_index: u32,
        #[source]
        source: crate::fragment::ParseError,
    },

    #[error("prepared document contains duplicate fragment `{path}`")]
    DuplicatePreparedFragment { path: FragmentPath },

    #[error(transparent)]
    Document(#[from] crate::adls::Error),

    #[error(transparent)]
    Commit(#[from] crate::commit::CommitError),

    #[error("memory could not be allocated while attempting to {operation}")]
    Allocation { operation: &'static str },
}

impl Error {
    /// External fragment paths fully written before an operational failure.
    pub fn committed_external(&self) -> &[FragmentPath] {
        match self {
            Self::Commit(error) => error.completed(),
            _ => &[],
        }
    }
}
