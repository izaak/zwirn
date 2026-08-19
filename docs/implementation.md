# Implementation

This document records implementation decisions as Zwirn develops. `docs/design.md` remains normative for observable behavior.

## Libraries

`clap` parses the command line. The command-line layer translates derived values into ordinary Zwirn values before calling library code, whose shape follows the domain independently of the parser.

`sha2` provides SHA-256. Hash parsing, formatting, and truncation produce a Zwirn-owned baseline-hash value, while SHA-2 digest types remain within the hashing implementation.

`thiserror` derives typed validation and operational errors. Unresolved synchronization states are ordinary results.

`cap-std` anchors fragment filesystem operations to one opened source-root directory and confines path resolution beneath it.

`tempfile` provides isolated workspaces for filesystem tests.

On macOS, `signal-hook` owns live-mode `SIGINT` and `SIGTERM` registration
through a blocking iterator with an explicit close handle. A dedicated thread
records the first signal in shared shutdown state and issues a bounded wake;
after native monitoring has stopped and drained, orderly teardown closes the
iterator to wake the thread, joins it, and drops the final delivery handle
normally. This unregisters the installed actions; the dispositions they
replaced remain effectively ignored through imminent process exit.

## Filesystem I/O

Zwirn uses whole-file reads and prepares outputs in memory.

Zwirn opens the source root once and retains its directory capability in the inventory through commit. Inventory reads and commit writes use canonical fragment paths relative to that same capability. The source-root spelling itself may contain symbolic links; opening it establishes the anchor. Containment does not exclude mounted subtrees or hard links to otherwise unmanaged files.

Discovery checks fragment-path component ancestry lexically, without inferring
relationships through case folding, Unicode normalization, symbolic-link
resolution, or other filesystem-specific behavior. To detect aliases among
managed inputs, it compares the device and inode of the opened document and
existing fragment targets. These identities are discarded after discovery and
are not revalidated before writing. Directory identities are not compared.

After exclusively creating an absent fragment, Zwirn derives its identity from the creation handle and probes the other targets that were absent during discovery through the source-root capability. Only a successful identity match is a collision; probe errors are ignored. A collision fails the commit after counting the new fragment as written, without rollback.

The lexical document-target check overlaps with identity comparison when a target is named exactly as the document. It remains as the direct expression of the named-path rule; identity comparison additionally detects aliases.

Existing fragment targets and the document use ordinary platform
create-or-truncate operations; absent fragment targets use exclusive creation.
Both have the platform's usual effects on destination metadata.

Complete document reads, fragment reads, fragment target writes, and document writes pass through a crate-private, statically dispatched access policy. The policy receives the lexical document path or the named fragment path derived from the retained source-root capability; those paths identify the access but do not replace capability-relative fragment mechanics. Parent validation and creation remain outside this boundary. Policy-access failure is represented separately from the unchanged result of an access body.

One-shot command execution selects the platform policy at compile time. macOS uses coordinated access; other targets use direct access. Direct access invokes each body once, synchronously, and cannot fail before the body runs, so it preserves the existing error chain.

On macOS, every policy access constructs a short-lived `NSFileCoordinator`
with no Zwirn-owned file presenter and uses the default read or write options.
The named fragment path is claimed even when its target does not exist.

Before invoking the filesystem body, the macOS policy compares the accessor's
filesystem representation with the configured named path. A mismatch fails
before the body runs, and neither it nor another pre-body coordination failure
is retried through direct access. The accessor does not replace the retained
capability-relative path used by a fragment body, so the body's existing
absence, creation, and alias logic remains in force.

A coordinated body runs synchronously and exactly once. Once invoked, its result is authoritative, so Rust selects its callback result before interpreting the native outcome. Coordination failures that occur before invocation are typed separately from body failures and retain the native error domain, code, and description when Foundation supplies them. A commit-side access failure also retains the fragment paths whose writes completed earlier in the ordered commit.

The Objective-C bridge owns the Foundation block and autorelease pool and catches Objective-C exceptions before returning through its C boundary. Rust catches callback panics before they can cross that boundary and retains callback state until the synchronous native call returns. Paths cross the bridge as filesystem-representation bytes rather than UTF-8 text, preserving non-UTF-8 paths.

The Objective-C source and its Foundation and Core Foundation linkage are
macOS-only. Its deployment target follows Rust's target rather than the host
SDK.

## ADLS source fields

PatchObject pool indexes identify distinct tables. This gives each node handle an independently rewritable object.

Rewriting appends replacement strings and redirects existing `f10` offsets. Audulus accepts this layout and compacts superseded strings when it next saves the document.

Before commit, the prepared document is reparsed as ADLS and its source fields
are reparsed for markers.

## Synchronization state

Classification distinguishes absent-file and matching-file forms of observable `unadopted` state because their safe actions differ. Unequal sources that both match the truncated baseline hash are treated conservatively as a conflict.

Command planning is pure. A forced selection is validated as a complete batch before actions are materialized.

## Live command integration

`clap` recognizes `live` on every supported target. The live-session module is
macOS-only; other targets dispatch the parsed command directly to the
unsupported result. The macOS binary passes the module the current directory
and configured paths.

Because the configured document spelling cannot change during a session, live
mode rejects a path without the `.audulus4` extension before it installs signal
handling or starts FSEvents. Checks whose outcomes can change at that path
remain in reconciliation.

For each reconciliation, the binary-private driver calls the public one-shot
engine with the fixed session paths, an empty selector list, and the ordinary
safe `sync` operation. Live wiring therefore adds no public library API or
parallel synchronization implementation.

## Live scheduling

The foreground driver executes reconciliations synchronously, without a
reconciliation worker. Signal handling and FSEvents monitoring are operational
before the initial call.

When idle, the driver blocks on its wake channel. During a reconciliation,
filesystem hints collapse into a capacity-one pending-dirty signal rather than
accumulating as event history. After the call returns, shutdown takes
precedence; otherwise, one pending signal orders an immediate follow-up.
Reconciliation outcomes do not enqueue retries.

## Live filesystem monitoring

The crate-private macOS monitoring boundary owns one FSEvents stream for the
source-root hierarchy and the configured document's parent. Relative inputs
are made absolute from one captured current directory without canonicalizing
them. The stream is created at `kFSEventStreamEventIdSinceNow`; event IDs are
neither exposed nor persisted, and the driver has no polling path.

FSEvents requires its roots as Core Foundation strings, while the existing command boundary admits filesystem-path bytes that Core Foundation may not represent. Monitor paths therefore cross the native boundary as byte slices, but each complete configured monitor scope must be representable as a Core Foundation filesystem path. The bridge does not truncate, canonicalize, resolve, or substitute another watch root: failed exact conversion is a path-representation startup failure before stream creation. One-shot command path support is unchanged. Exact duplicate watch roots share one stream entry.

The stream requests file events, watched-root notifications, prompt first-burst
delivery, and own-event marking; it does not suppress own writes. Every
nonempty native callback batch makes a nonblocking send to the capacity-one
invalidation channel described above. A full channel already represents
pending work, and a disconnected channel represents shutdown. The callback
discards event paths, flags, IDs, counts, and ordering, so special conditions
such as dropped events and watched-root changes have the same full-resample
meaning as ordinary hints. FSEvents exposes no general post-start stream-failure
callback, so the boundary does not invent a runtime-health state.

Construction succeeds only after `FSEventStreamStart` returns true. Creation,
queue, or start failure releases all partial native state. The native monitor
borrows a boxed Rust sender only while the stream is alive. Stop first prevents
further delivery, invalidates the stream, drains its private serial dispatch
queue, and releases the stream and queue; only then is the Rust callback
allocation released. The FSEvents bridge uses only C APIs, and the Rust
callback trampoline catches panics before returning through C. Its C source
and CoreServices linkage are macOS-only.

## Live reporting

The driver retains the previous reconciliation outcome to suppress unchanged
unresolved results and repeated blocker diagnostics. Consecutive successful
no-action reconciliations are silent. Completed external writes retained by an
engine failure are reported before the blocker.
