use zwirn::fragment::{BaselineHash, CanonicalSource};
use zwirn::reconcile::{
    Action, Classification, Decision, Operation, PlanError, SelectionKind, State, UnadoptedFile,
    classify, plan,
};

fn source(value: &str) -> CanonicalSource {
    CanonicalSource::try_from(value).unwrap()
}

#[test]
fn classifies_every_semantic_relationship() {
    let baseline_source = source("baseline");
    let baseline = BaselineHash::from_source(&baseline_source);
    let first_change = source("first change");
    let second_change = source("second change");

    let cases = [
        (
            &baseline_source,
            None,
            None,
            Classification::Unadopted(UnadoptedFile::Absent),
        ),
        (
            &baseline_source,
            None,
            Some(&baseline_source),
            Classification::Unadopted(UnadoptedFile::Matching),
        ),
        (
            &baseline_source,
            None,
            Some(&first_change),
            Classification::UnadoptedConflict,
        ),
        (
            &baseline_source,
            Some(baseline),
            None,
            Classification::Missing,
        ),
        (
            &baseline_source,
            Some(baseline),
            Some(&baseline_source),
            Classification::Synchronized,
        ),
        (
            &baseline_source,
            Some(baseline),
            Some(&first_change),
            Classification::Embed,
        ),
        (
            &first_change,
            Some(baseline),
            Some(&baseline_source),
            Classification::Extract,
        ),
        (
            &first_change,
            Some(baseline),
            Some(&first_change),
            Classification::Converged,
        ),
        (
            &first_change,
            Some(baseline),
            Some(&second_change),
            Classification::Conflict,
        ),
    ];

    for (embedded, baseline, filesystem, expected) in cases {
        assert_eq!(classify(embedded, baseline, filesystem), expected);
    }
}

#[test]
fn plans_normal_commands_from_the_state_table() {
    use Action::{Embed as EmbedAction, Extract as ExtractAction, Record};
    use Classification::{
        Conflict, Converged, Embed, Extract, Missing, Synchronized, Unadopted, UnadoptedConflict,
    };
    use Decision::{Act, Settled};

    let classifications = [
        Unadopted(UnadoptedFile::Absent),
        Unadopted(UnadoptedFile::Matching),
        UnadoptedConflict,
        Missing,
        Synchronized,
        Embed,
        Extract,
        Converged,
        Conflict,
    ];
    let unresolved = |state| Decision::Unresolved(state);
    let cases = [
        (
            Operation::Embed { force: false },
            [
                unresolved(State::Unadopted),
                Act(Record),
                unresolved(State::UnadoptedConflict),
                unresolved(State::Missing),
                Settled,
                Act(EmbedAction),
                unresolved(State::Extract),
                Act(Record),
                unresolved(State::Conflict),
            ],
        ),
        (
            Operation::Extract { force: false },
            [
                Act(ExtractAction),
                Act(Record),
                unresolved(State::UnadoptedConflict),
                unresolved(State::Missing),
                Settled,
                unresolved(State::Embed),
                Act(ExtractAction),
                Act(Record),
                unresolved(State::Conflict),
            ],
        ),
        (
            Operation::Sync,
            [
                Act(ExtractAction),
                Act(Record),
                unresolved(State::UnadoptedConflict),
                unresolved(State::Missing),
                Settled,
                Act(EmbedAction),
                Act(ExtractAction),
                Act(Record),
                unresolved(State::Conflict),
            ],
        ),
    ];

    for (operation, expected) in cases {
        assert_eq!(
            plan(operation, SelectionKind::All, &classifications).unwrap(),
            expected
        );
    }

    assert_eq!(
        plan(
            Operation::Extract { force: false },
            SelectionKind::Explicit,
            &[Classification::Missing],
        )
        .unwrap(),
        [Decision::Act(Action::Extract)]
    );
    assert_eq!(
        plan(
            Operation::Sync,
            SelectionKind::Explicit,
            &[Classification::Missing],
        )
        .unwrap(),
        [Decision::Unresolved(State::Missing)]
    );
}

#[test]
fn validates_a_forced_batch_before_returning_actions() {
    assert_eq!(
        plan(
            Operation::Embed { force: true },
            SelectionKind::All,
            &[Classification::Conflict],
        ),
        Err(PlanError::ForceRequiresExplicitSelection)
    );
    assert_eq!(
        plan(
            Operation::Extract { force: true },
            SelectionKind::Explicit,
            &[Classification::Conflict, Classification::Embed],
        ),
        Err(PlanError::InvalidForcedState {
            index: 1,
            state: State::Embed,
        })
    );

    assert_eq!(
        plan(
            Operation::Embed { force: true },
            SelectionKind::Explicit,
            &[Classification::UnadoptedConflict, Classification::Conflict,],
        )
        .unwrap(),
        [Decision::Act(Action::Embed), Decision::Act(Action::Embed)]
    );
    assert_eq!(
        plan(
            Operation::Extract { force: true },
            SelectionKind::Explicit,
            &[Classification::Conflict],
        )
        .unwrap(),
        [Decision::Act(Action::Extract)]
    );
}
