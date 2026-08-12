# Implementation

This document records implementation decisions as Zwirn develops. `docs/design.md` remains normative for observable behavior.

## Libraries

`clap` parses the command line. The command-line layer translates derived values into ordinary Zwirn values before calling library code, whose shape follows the domain independently of the parser.

`sha2` provides SHA-256. Hash parsing, formatting, and truncation produce a Zwirn-owned baseline-hash value, while SHA-2 digest types remain within the hashing implementation.

`thiserror` derives typed validation and operational errors. Unresolved synchronization states are ordinary results.

`tempfile` provides isolated workspaces for filesystem tests.

## Filesystem I/O

Zwirn uses whole-file reads and prepares outputs in memory.

Fragment uniqueness is exact canonical marker-path equality. Distinct marker paths are otherwise independent. Zwirn does not compare resolved destinations or filesystem identities.

Direct writes use ordinary platform file creation, truncation, and write semantics, including their effects on destination metadata.

## ADLS source fields

PatchObject pool indexes identify distinct tables. This gives each node handle an independently rewritable object.

Rewriting appends replacement strings and redirects existing `f10` offsets. Audulus accepts this layout and compacts superseded strings when it next saves the document.

## Synchronization state

The synchronization-state representation is deferred until the surrounding inputs, outputs, and orchestration boundaries are established.
