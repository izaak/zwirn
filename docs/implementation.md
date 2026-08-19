# Implementation

This document records implementation decisions as Zwirn develops. `docs/design.md` remains normative for observable behavior.

## Libraries

`clap` parses the command line. The command-line layer translates derived values into ordinary Zwirn values before calling library code, whose shape follows the domain independently of the parser.

`sha2` provides SHA-256. Hash parsing, formatting, and truncation produce a Zwirn-owned baseline-hash value, while SHA-2 digest types remain within the hashing implementation.

`thiserror` derives typed validation and operational errors. Unresolved synchronization states are ordinary results.

`cap-std` anchors fragment filesystem operations to one opened source-root directory and confines path resolution beneath it.

`tempfile` provides isolated workspaces for filesystem tests.

`signal-hook` owns macOS live-mode `SIGINT` and `SIGTERM` registration through
a blocking iterator with an explicit close handle. A dedicated iterator thread
records the first signal in shared shutdown state and issues a bounded wake;
orderly teardown closes and joins that thread while leaving signal dispositions
captured through imminent process exit. It is compiled only for macOS, so
Linux's unsupported live command does not acquire signal machinery.

## Filesystem I/O

Zwirn uses whole-file reads and prepares outputs in memory.

Zwirn opens the source root once and retains its directory capability in the inventory through commit. Inventory reads and commit writes use canonical fragment paths relative to that same capability. The source-root spelling itself may contain symbolic links; opening it establishes the anchor. Containment does not exclude mounted subtrees or hard links to otherwise unmanaged files.

Fragment uniqueness is exact canonical marker-path equality. Separately, discovery rejects a canonical marker path that is a strict component ancestor of another because both cannot be regular-file targets. It does not infer component ancestry through case folding, Unicode normalization, symbolic-link resolution, or other filesystem-specific relationships. During discovery, Zwirn compares the device and inode of the opened document and existing fragment targets to reject aliases among managed inputs. These identities are discarded after discovery and are not revalidated before writing. Directory identities are not compared.

After exclusively creating an absent fragment, Zwirn derives its identity from the creation handle and probes the other targets that were absent during discovery through the source-root capability. Only a successful identity match is a collision; probe errors are ignored. A collision fails the commit after counting the new fragment as written, without rollback.

The lexical document-target check overlaps with identity comparison when a target is named exactly as the document. It remains as the direct expression of the named-path rule; identity comparison additionally detects aliases.

Existing fragment targets and the document use ordinary platform create-or-truncate writes. Fragment targets absent during discovery use exclusive creation. Both use ordinary platform write behavior, including its effects on destination metadata.

Complete document reads, fragment reads, fragment target writes, and document writes pass through a crate-private, statically dispatched access policy. The policy receives the lexical document path or the named fragment path derived from the retained source-root capability; those paths identify the access but do not replace capability-relative fragment mechanics. Parent validation and creation remain outside this boundary. Policy-access failure is represented separately from the unchanged result of an access body.

One-shot command execution selects the platform policy at compile time. macOS uses coordinated access; other targets use direct access. Direct access invokes each body once, synchronously, and cannot fail before the body runs, so it preserves the existing error chain.

On macOS, every policy access constructs a short-lived `NSFileCoordinator` with no Zwirn-owned file presenter and uses the default read or write options. The named fragment path is claimed even when its target does not exist. A failure to establish coordinated access is returned without retrying the operation through direct access.

The macOS policy accepts an accessor only when its filesystem representation matches the configured named path. It rejects a changed accessor path before invoking the filesystem body and never substitutes that path for the retained capability-relative fragment operation. Ordinary absence, exclusive creation, and alias detection therefore remain properties of the existing body.

A coordinated body runs synchronously and exactly once. Once invoked, its result is authoritative, so Rust selects its callback result before interpreting the native outcome. Coordination failures that occur before invocation are typed separately from body failures and retain the native error domain, code, and description when Foundation supplies them. A commit-side access failure also retains the fragment paths whose writes completed earlier in the ordered commit.

The Objective-C bridge owns the Foundation block and autorelease pool and catches Objective-C exceptions before returning through its C boundary. Rust catches callback panics before they can cross that boundary and retains callback state until the synchronous native call returns. Paths cross the bridge as filesystem-representation bytes rather than UTF-8 text, preserving non-UTF-8 paths.

The Objective-C source is compiled and the Foundation and Core Foundation frameworks are linked only for macOS targets. Its deployment target follows Rust's target rather than the host SDK. Non-macOS builds do not require Apple frameworks or an Objective-C toolchain.

## ADLS source fields

PatchObject pool indexes identify distinct tables. This gives each node handle an independently rewritable object.

Rewriting appends replacement strings and redirects existing `f10` offsets. Audulus accepts this layout and compacts superseded strings when it next saves the document.

## Synchronization state

Classification distinguishes absent-file and matching-file forms of observable `unadopted` state because their safe actions differ. Unequal sources that both match the truncated baseline hash are treated conservatively as a conflict.

Command planning is pure. A forced selection is validated as a complete batch before actions are materialized.

## Live scheduling

Live-session scheduling is crate-private. The foreground driver executes each
full-inventory reconciliation synchronously. Filesystem invalidations and
shutdown requests remain bounded, level-triggered state while it runs rather
than accumulating as event histories.

Signal handling and FSEvents monitoring are operational before the immediate
initial reconciliation begins. After a reconciliation returns, a pending
shutdown prevents later work; otherwise, a pending invalidation requests one
subsequent reconciliation. Further invalidations may merge into that same
request. Reconciliations therefore remain serialized without a separate
reconciliation worker.

The driver may briefly coalesce invalidations, but it does not preserve an
exact batching interval or event count. Reconciliation outcomes do not request
their own retry: after startup, another run requires a filesystem invalidation.

## Live filesystem monitoring

The macOS monitoring boundary is crate-private and owns one FSEvents stream for the fixed source-root hierarchy and the fixed parent of the configured document path. Relative inputs are made absolute from one captured current directory without canonicalizing them. The stream is created at `kFSEventStreamEventIdSinceNow`; Zwirn neither exposes nor persists event IDs and has no monitoring fallback or polling path.

FSEvents requires its roots as Core Foundation strings, while the existing command boundary admits filesystem-path bytes that Core Foundation may not represent. Monitor paths therefore cross the native boundary as byte slices, but each complete configured monitor scope must be representable as a Core Foundation filesystem path. The bridge does not truncate, canonicalize, resolve, or substitute another watch root: failed exact conversion is a path-representation startup failure before stream creation. One-shot command path support is unchanged. Exact duplicate watch roots share one stream entry.

The stream requests file events, watched-root notifications, prompt first-burst delivery, and own-event marking; it does not suppress own writes. Every nonempty native callback batch attempts to set one capacity-one Rust invalidation latch that exists before stream startup. Delivery never blocks: an empty latch accepts the hint, a full latch already represents the same pending full resample, and a disconnected latch represents shutdown. Event paths, flags, IDs, counts, and ordering are discarded. Dropped-event, rescan-required, ID-wrap, mount, and watched-root-change indications therefore cannot be filtered out and have the same level-triggered full-resample meaning as ordinary hints. FSEvents exposes no general post-start stream-failure callback, so the boundary does not invent a runtime-health state.

Construction succeeds only after `FSEventStreamStart` returns true. Creation, queue, or start failure releases all partial native state and returns without another mechanism. The native monitor borrows a boxed Rust sender only while the stream is alive. Stop first prevents further delivery, invalidates the stream, drains its private serial dispatch queue, and releases the stream and queue; only after native stop returns is the Rust callback allocation released. The FSEvents bridge uses only C APIs, and the Rust callback trampoline catches panics before returning through C. Apple source compilation and CoreServices linkage remain inside the existing macOS target guard.

## Command reporting

Results are path-first, tab-separated lines. Mutating commands omit already synchronized fragments and use `record`, `embed`, and `extract` for performed actions. Diagnostics use standard error.
