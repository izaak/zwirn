# Target design: `zwirn live`

Except where this document changes it, the observable behavior defined in
`docs/design.md` remains part of the target design.

## Command and scope

On macOS, the live command is:

```text
zwirn live [--source-root DIR] DOCUMENT
```

The document and source root have the same resolution and validation rules as
the existing commands. A live session operates on one document and its entire
freshly discovered fragment inventory. It has no fragment selectors,
direction, force, daemon, or output-format options.

Each reconciliation applies the existing safe, bidirectional `sync` behavior.
It does not force conflicts or add deletion as a synchronization operation. In
particular, an established fragment whose external file is absent remains
`missing`; live mode reports it and does not recreate it automatically.

The configured document and source-root paths remain fixed for the session. A
document replaced at the same named path remains in scope, but live mode does
not follow a document or source root moved to another path.

On Linux, `live` remains visible in the command-line interface but reports that
it is unsupported and exits with status 2. Existing one-shot behavior remains
supported.

## Coordinated access

On macOS, live mode uses the coordinated document and fragment access defined
for the one-shot commands in `docs/design.md`.

## Monitoring and reconciliation

Live mode watches the source-root hierarchy and the parent of the configured
document. It starts monitoring before performing an immediate initial
reconciliation, so the initial run recovers changes made before startup without
leaving a gap in which a new change can be missed.

Filesystem events are invalidation hints, not an ordered history of changes.
A relevant hint requests a fresh reconciliation of the document and its full
fragment inventory. Implementations may filter obviously unrelated hints and
may reconcile more often than strictly necessary; event delivery and batching
are not user-visible transactions.

Reconciliations are serialized. The first pending hint starts a short
coalescing window; later hints join that window without postponing its end. The
exact interval is not a timing guarantee. A hint that arrives during a
reconciliation guarantees a subsequent reconciliation rather than being
absorbed into the active run.

The foreground session starts its FSEvents stream from the current event
position. It has no routine polling path and persists no event cursor. Dropped
event and watched-root-change indications request a full reconciliation. If a
usable event stream cannot be established or maintained, live mode reports the
failure and exits with status 2.

Once monitoring is operational, a reconciliation blocker does not terminate
the session. Live mode reports the blocker and remains responsive to relevant
filesystem changes. A later change triggers a fresh reconciliation, allowing a
user to recover, for example, by correcting and saving a malformed fragment.
This behavior promises no periodic or timed retry in the absence of a new
filesystem hint.

## Diagnostics and shutdown

Live diagnostics are for the foreground human workflow, not a stable
machine-readable protocol. They report meaningful session changes, including
startup, performed actions, blockers, recovery from blockers, and shutdown.
Routine reconciliations that perform no action and do not change the reported
state remain silent. Exact wording and line structure are not part of the
target contract.

`SIGINT` and `SIGTERM` request an orderly shutdown. An idle session stops
promptly. An active synchronous reconciliation finishes, no further
reconciliation begins, and the process then exits with status 0. Live mode does
not add cancellation inside reconciliation.

## First-version limits

The first version has no Zwirn-owned file presenter, multi-document session,
persistent configuration, daemon or control process, LaunchAgent packaging,
status IPC, durable logging, persistent event history, polling fallback, or
dirty-buffer integration. Those are not prerequisites for foreground live
mode and require separate evidence and design if considered later.
