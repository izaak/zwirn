//! Pure synchronization classification and command planning.

use std::fmt;

use thiserror::Error;

use crate::fragment::{BaselineHash, CanonicalSource};

/// A synchronization state reported at the command-line interface.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum State {
    Unadopted,
    UnadoptedConflict,
    Missing,
    Synchronized,
    Embed,
    Extract,
    Converged,
    Conflict,
}

impl State {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unadopted => "unadopted",
            Self::UnadoptedConflict => "unadopted conflict",
            Self::Missing => "missing",
            Self::Synchronized => "synchronized",
            Self::Embed => "embed",
            Self::Extract => "extract",
            Self::Converged => "converged",
            Self::Conflict => "conflict",
        }
    }
}

impl fmt::Display for State {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A classified fragment, including the hidden distinction needed for adoption.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Classification {
    Unadopted(UnadoptedFile),
    UnadoptedConflict,
    Missing,
    Synchronized,
    Embed,
    Extract,
    Converged,
    Conflict,
}

impl Classification {
    pub const fn state(self) -> State {
        match self {
            Self::Unadopted(_) => State::Unadopted,
            Self::UnadoptedConflict => State::UnadoptedConflict,
            Self::Missing => State::Missing,
            Self::Synchronized => State::Synchronized,
            Self::Embed => State::Embed,
            Self::Extract => State::Extract,
            Self::Converged => State::Converged,
            Self::Conflict => State::Conflict,
        }
    }
}

/// Filesystem detail hidden by the observable `unadopted` state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnadoptedFile {
    Absent,
    Matching,
}

/// Classifies validated canonical embedded and filesystem sources.
pub fn classify(
    embedded: &CanonicalSource,
    baseline: Option<BaselineHash>,
    filesystem: Option<&CanonicalSource>,
) -> Classification {
    let Some(baseline) = baseline else {
        return match filesystem {
            None => Classification::Unadopted(UnadoptedFile::Absent),
            Some(filesystem) if filesystem == embedded => {
                Classification::Unadopted(UnadoptedFile::Matching)
            }
            Some(_) => Classification::UnadoptedConflict,
        };
    };
    let Some(filesystem) = filesystem else {
        return Classification::Missing;
    };

    let filesystem_matches = BaselineHash::from_source(filesystem) == baseline;
    let embedded_matches = BaselineHash::from_source(embedded) == baseline;
    if filesystem == embedded {
        if filesystem_matches {
            Classification::Synchronized
        } else {
            Classification::Converged
        }
    } else {
        match (filesystem_matches, embedded_matches) {
            (false, true) => Classification::Embed,
            (true, false) => Classification::Extract,
            // `(true, true)` requires a collision in the truncated hash.
            // Neither unequal-source case has an unambiguous direction.
            (false, false) | (true, true) => Classification::Conflict,
        }
    }
}

/// A mutating synchronization operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Operation {
    Embed { force: bool },
    Extract { force: bool },
    Sync,
}

impl Operation {
    const fn forced_action(self) -> Option<Action> {
        match self {
            Self::Embed { force: true } => Some(Action::Embed),
            Self::Extract { force: true } => Some(Action::Extract),
            Self::Embed { force: false } | Self::Extract { force: false } | Self::Sync => None,
        }
    }
}

/// Whether selectors named exact paths or selected the complete inventory.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SelectionKind {
    All,
    Explicit,
}

/// A source or baseline mutation selected by the planner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    Record,
    Embed,
    Extract,
}

impl Action {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Record => "record",
            Self::Embed => "embed",
            Self::Extract => "extract",
        }
    }
}

impl fmt::Display for Action {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// What a mutating command should do with one selected fragment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Decision {
    Settled,
    Act(Action),
    Unresolved(State),
}

/// Plans a selected batch without performing effects.
///
/// A forced batch is validated in full before any actions are returned.
pub fn plan(
    operation: Operation,
    selection: SelectionKind,
    selected: &[Classification],
) -> Result<Vec<Decision>, PlanError> {
    if let Some(action) = operation.forced_action() {
        if selection != SelectionKind::Explicit || selected.is_empty() {
            return Err(PlanError::ForceRequiresExplicitSelection);
        }
        if let Some((index, classification)) =
            selected
                .iter()
                .copied()
                .enumerate()
                .find(|(_, classification)| {
                    !matches!(
                        classification,
                        Classification::Conflict | Classification::UnadoptedConflict
                    )
                })
        {
            return Err(PlanError::InvalidForcedState {
                index,
                state: classification.state(),
            });
        }

        let mut decisions = reserve_plan(selected.len())?;
        decisions.extend(std::iter::repeat_n(Decision::Act(action), selected.len()));
        return Ok(decisions);
    }

    let mut decisions = reserve_plan(selected.len())?;
    decisions.extend(
        selected
            .iter()
            .copied()
            .map(|classification| plan_one(operation, selection, classification)),
    );
    Ok(decisions)
}

fn reserve_plan(length: usize) -> Result<Vec<Decision>, PlanError> {
    let mut decisions = Vec::new();
    decisions
        .try_reserve_exact(length)
        .map_err(|_| PlanError::AllocationFailed)?;
    Ok(decisions)
}

fn plan_one(
    operation: Operation,
    selection: SelectionKind,
    classification: Classification,
) -> Decision {
    use Action::{Embed as EmbedAction, Extract as ExtractAction, Record};
    use Classification::{Converged, Embed, Extract, Missing, Synchronized, Unadopted};

    let action = match (operation, classification) {
        (_, Synchronized) => return Decision::Settled,

        (_, Unadopted(UnadoptedFile::Matching) | Converged) => Record,

        (
            Operation::Extract { force: false } | Operation::Sync,
            Unadopted(UnadoptedFile::Absent),
        )
        | (Operation::Extract { force: false } | Operation::Sync, Extract) => ExtractAction,

        (Operation::Extract { force: false }, Missing) if selection == SelectionKind::Explicit => {
            ExtractAction
        }

        (Operation::Embed { force: false } | Operation::Sync, Embed) => EmbedAction,

        (
            Operation::Embed { force: false }
            | Operation::Extract { force: false }
            | Operation::Sync,
            _,
        ) => {
            return Decision::Unresolved(classification.state());
        }

        (Operation::Embed { force: true } | Operation::Extract { force: true }, _) => {
            unreachable!("forced operations are planned as a validated batch")
        }
    };
    Decision::Act(action)
}

/// A command-planning validation failure.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum PlanError {
    #[error("--force requires at least one explicitly selected fragment")]
    ForceRequiresExplicitSelection,

    #[error("selected fragment at index {index} is in `{state}` state, which cannot be forced")]
    InvalidForcedState { index: usize, state: State },

    #[error("memory could not be reserved for the synchronization plan")]
    AllocationFailed,
}
