# Implementation

This document records implementation decisions as Zwirn develops. `docs/design.md` remains normative for observable behavior.

## Libraries

`clap` parses the command line. The command-line layer translates derived values into ordinary Zwirn values before calling library code, whose shape follows the domain independently of the parser.

`sha2` provides SHA-256. Hash parsing, formatting, and truncation produce a Zwirn-owned baseline-hash value, while SHA-2 digest types remain within the hashing implementation.

`thiserror` derives typed validation and operational errors. Unresolved synchronization states are ordinary results.

`cap-std` anchors fragment filesystem operations to one opened source-root directory and confines path resolution beneath it.

`tempfile` provides isolated workspaces for filesystem tests.

## Filesystem I/O

Zwirn uses whole-file reads and prepares outputs in memory.

Zwirn opens the source root once and retains its directory capability in the inventory through commit. Inventory reads and commit writes use canonical fragment paths relative to that same capability. The source-root spelling itself may contain symbolic links; opening it establishes the anchor. Containment does not exclude mounted subtrees or hard links to otherwise unmanaged files.

Fragment uniqueness is exact canonical marker-path equality. Separately, discovery rejects a canonical marker path that is a strict component ancestor of another because both cannot be regular-file targets. It does not infer component ancestry through case folding, Unicode normalization, symbolic-link resolution, or other filesystem-specific relationships. During discovery, Zwirn compares the device and inode of the opened document and existing fragment targets to reject aliases among managed inputs. These identities are discarded after discovery and are not revalidated before writing. Directory identities are not compared.

After exclusively creating an absent fragment, Zwirn derives its identity from the creation handle and probes the other targets that were absent during discovery through the source-root capability. Only a successful identity match is a collision; probe errors are ignored. A collision fails the commit after counting the new fragment as written, without rollback.

The lexical document-target check overlaps with identity comparison when a target is named exactly as the document. It remains as the direct expression of the named-path rule; identity comparison additionally detects aliases.

Existing fragment targets and the document use ordinary platform create-or-truncate writes. Fragment targets absent during discovery use exclusive creation. Both use ordinary platform write behavior, including its effects on destination metadata.

Complete document reads, fragment reads, fragment target writes, and document writes pass through a crate-private, statically dispatched access policy. The policy receives the lexical document path or the named fragment path derived from the retained source-root capability; those paths identify the access but do not replace capability-relative fragment mechanics. Parent validation and creation remain outside this boundary. Policy-access failure is represented separately from the unchanged result of an access body.

All current entry points select direct access. It invokes each body once, synchronously, and cannot fail before the body runs, so the policy boundary does not add observable errors or change existing error chains.

## ADLS source fields

PatchObject pool indexes identify distinct tables. This gives each node handle an independently rewritable object.

Rewriting appends replacement strings and redirects existing `f10` offsets. Audulus accepts this layout and compacts superseded strings when it next saves the document.

## Synchronization state

Classification distinguishes absent-file and matching-file forms of observable `unadopted` state because their safe actions differ. Unequal sources that both match the truncated baseline hash are treated conservatively as a conflict.

Command planning is pure. A forced selection is validated as a complete batch before actions are materialized.

## Command reporting

Results are path-first, tab-separated lines. Mutating commands omit already synchronized fragments and use `record`, `embed`, and `extract` for performed actions. Diagnostics use standard error.
