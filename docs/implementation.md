# Implementation

This document records implementation decisions as Zwirn develops.
[design.md](design.md) remains normative for observable behavior.

## Domain boundaries

The command-line layer converts parser-specific values into parser-independent
domain values before calling library code. Baseline hashes likewise use a
Zwirn-owned type; SHA-2 digest types remain inside the hashing implementation.
Validation and operational failures are typed errors, while unresolved
synchronization states are ordinary results.

## Synchronization engine

The engine uses whole-file reads and prepares outputs in memory. Classification
keeps two internal forms of `unadopted`: one for an absent filesystem file and
one for matching filesystem and embedded source. They share an observable state
but have different safe actions. Because the baseline stores only a 64-bit hash
prefix, unequal sources can both match it; classification treats that collision
as a conflict.

Command planning has no side effects. For a forced command, the engine validates
the entire selection before constructing any actions.

## Filesystem I/O

### Containment and identity

`cap-std` opens the source root once as a directory capability: a directory
handle that anchors relative access. The inventory retains that handle through
commit, and every fragment read and write uses a path relative to it. Opening
fixes the anchor even when the supplied source-root path traverses symbolic links;
containment does not exclude mounted subtrees or hard links to files outside
the tree.

Component ancestry is a property of marker path spellings. It is checked
without case folding, Unicode normalization, or symbolic-link inference.
During discovery, Zwirn compares the opened document and existing fragment
targets by device and inode, rejecting aliases. It does not revalidate those
identities before commit, and it does not identity-check directories.

After exclusively creating a previously absent fragment, commit takes the new
file's identity from its creation handle and compares it with other targets
that were absent during discovery. Failure to probe one of those paths is
ignored. A detected collision fails only after the new fragment has been
written.

### Access policy

Complete file reads and writes pass through an internal, statically dispatched
access policy. Named paths tell the policy which access to coordinate, while
fragment filesystem operations retain their capability-relative paths.
Parent-directory handling remains outside the policy. Engine execution selects
coordinated access on macOS and direct access on other targets.

The macOS policy constructs a short-lived `NSFileCoordinator` without a
Zwirn-owned file presenter and uses default read and write options. It requests
coordination even for fragment paths that do not yet exist. If the coordinator
supplies an accessor path different from the requested path or otherwise fails
before the filesystem operation begins, Zwirn does not retry through direct
access. It never substitutes that accessor path for a fragment's
capability-relative path.

Coordination failures that occur before a filesystem operation remain distinct
from failures returned by the operation itself. Once the operation begins, its
result is authoritative; commit failures retain the paths written earlier. The
Objective-C bridge catches Objective-C exceptions and Rust panics before either
can cross the language boundary. It represents paths as filesystem bytes so
non-UTF-8 paths survive the bridge. The bridge and its Foundation and Core
Foundation linkage are macOS-only; its deployment target follows the Rust
target rather than the host SDK.

## ADLS source fields

Each entry in the ADLS root `PatchObject` vector has a document-local index.
Zwirn uses that index as a node handle and rejects repeated table references, so
every accepted handle names a distinct, independently rewritable object.

To replace source, Zwirn appends new string data and redirects the object's
`f10` source field rather than rebuilding the FlatBuffer. Audulus compacts
superseded strings when it next saves the document. Before commit, Zwirn
reparses the prepared document and its marker structure. The focused binary
layout is in the [ADLS code reference](../reference/adls-code.md).

## Live mode

### Platform and reuse

The CLI recognizes `live` on every target, but the private live-session module
and its macOS-specific dependencies compile only on macOS.

Each reconciliation calls the public one-shot engine with the session's fixed
document and source-root paths, no selectors, and safe `sync`; live mode has no
parallel synchronization implementation. The document-extension check runs
before the long-lived session machinery starts, while checks whose outcomes can
change remain inside each reconciliation.

### Scheduling

The foreground driver calls the engine synchronously. Signal handling and
FSEvents monitoring are active before the initial call. The wake channel holds
at most one pending filesystem invalidation, so bursts collapse into one wake.
A separate atomic shutdown flag ensures a full wake channel cannot lose a
shutdown request. Shutdown takes precedence after a reconciliation, and
reconciliation outcomes do not enqueue retries.

### Filesystem monitoring

One FSEvents stream watches the source-root hierarchy and the configured
document's parent. Relative watch paths are made absolute from one captured
current directory without canonicalization. The stream starts from FSEvents'
current event position; event IDs are not persisted, and there is no polling
fallback.

FSEvents roots must be exactly representable as Core Foundation filesystem
paths. An unrepresentable root fails live-mode startup; one-shot path support
is unchanged. Native event metadata is discarded, events caused by Zwirn's own
writes are not suppressed, and every nonempty callback batch becomes the same
nonblocking invalidation. FSEvents has no general post-start failure callback,
so Zwirn does not track whether the stream remains healthy after startup.

The monitoring bridge keeps the callback's Rust state alive for the stream
lifetime. During teardown, it stops and drains event delivery before releasing
that state. It also catches Rust panics at the C boundary.

### Reporting

The live reporter remembers previous blocker state to suppress unchanged
attention and repeated failure diagnostics. If an engine call fails after
completing external writes, the reporter lists those writes before the blocker.
