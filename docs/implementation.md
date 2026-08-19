# Implementation

This document records implementation decisions as Zwirn develops. `docs/design.md` remains normative for observable behavior.

## Domain boundaries

The command-line layer converts parser-derived values into ordinary domain
values before calling library code. Baseline hashes likewise use a Zwirn-owned
type; SHA-2 digest types remain inside the hashing implementation. Validation
and operational failures are typed errors, while unresolved synchronization
states are ordinary results.

## Synchronization engine

The engine uses whole-file reads and prepares outputs in memory. Classification
distinguishes the absent-file and matching-file forms of `unadopted` because
their safe actions differ. Unequal sources that both match the truncated
baseline hash are treated conservatively as a conflict.

Command planning is pure. A forced selection is validated as a complete batch
before actions are materialized.

## Filesystem I/O

`cap-std` opens the source root once, and the inventory retains that directory
capability through commit. Fragment reads and writes remain relative to the
same capability. Opening establishes the anchor even when the source-root
spelling contains symbolic links; containment does not exclude mounted
subtrees or hard links to files outside the tree.

Component ancestry is checked lexically, without case folding, Unicode
normalization, or symbolic-link inference. Existing managed files are compared
by device and inode during discovery, but those identities are not revalidated
before commit and directories are not identity-checked.

After exclusively creating a previously absent fragment, commit compares its
identity with the other targets that were absent during discovery. Probe errors
are ignored. A detected collision fails only after the new fragment has been
written.

Complete file reads and writes pass through a crate-private, statically
dispatched access policy. Named paths identify coordinated accesses, while
fragment bodies retain their capability-relative paths and parent-directory
handling remains outside the policy. macOS selects coordinated access; other
targets select direct access.

The macOS policy constructs a short-lived `NSFileCoordinator` without a
Zwirn-owned file presenter and claims fragment paths even when they do not yet
exist. A changed accessor or another failure before body invocation is not
retried through direct access, and the accessor never replaces a fragment's
capability-relative path.

Pre-body coordination failures remain distinct from filesystem-body failures.
Once the body is invoked, its result is authoritative; commit failures retain
the paths written earlier. The native bridge prevents Objective-C exceptions
and Rust panics from crossing the language boundary, and passes paths as
filesystem-representation bytes so non-UTF-8 paths survive it. Its deployment
target follows the Rust target rather than the host SDK.

## ADLS source fields

PatchObject pool indexes identify distinct tables, giving each node handle an
independently rewritable object. Rewriting appends replacement strings and
redirects existing `f10` offsets; Audulus compacts the superseded strings when
it next saves the document. Before commit, the prepared document and its marker
structure are reparsed.

## Live mode

The CLI recognizes `live` on every target, but the private live-session module
and its Apple dependencies compile only on macOS. Each reconciliation calls the
public one-shot engine with the fixed session paths, no selectors, and safe
`sync`; live mode has no parallel synchronization implementation. The fixed
document extension is checked before the long-lived session machinery starts,
while checks whose outcomes can change remain in reconciliation.

The foreground driver calls the engine synchronously. Signal handling and
FSEvents monitoring are active before the initial call. A capacity-one wake
channel collapses filesystem invalidations, while a separate atomic shutdown
flag ensures a full channel cannot lose a control request. Shutdown takes
precedence after a reconciliation, and reconciliation outcomes do not enqueue
retries.

One FSEvents stream watches the source-root hierarchy and the configured
document's parent. Relative scopes are made absolute from one captured current
directory without canonicalization. The stream starts from the current event
position; event IDs are not persisted and there is no polling path.

FSEvents roots must be exactly representable as Core Foundation filesystem
paths. An unrepresentable root fails live-mode startup; one-shot path support
is unchanged. Native event metadata is discarded, own events are not
suppressed, and every nonempty callback batch becomes the same nonblocking
invalidation. FSEvents has no general post-start failure callback, so Zwirn
does not maintain a general stream-health state after startup.

The monitoring bridge retains callback state for the stream lifetime, drains
delivery before releasing it, and catches Rust panics at the C boundary.

The live reporter retains previous blocker state to suppress unchanged
attention and repeated failure diagnostics. Completed external writes from a
failed engine call are reported before its blocker.
