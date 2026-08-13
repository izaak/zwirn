# Implementation

This document records implementation decisions as Zwirn develops. `docs/design.md` remains normative for observable behavior.

## Libraries

`clap` parses the command line. The command-line layer translates derived values into ordinary Zwirn values before calling library code, whose shape follows the domain independently of the parser.

`sha2` provides SHA-256. Hash parsing, formatting, and truncation produce a Zwirn-owned baseline-hash value, while SHA-2 digest types remain within the hashing implementation.

`thiserror` derives typed validation and operational errors. Unresolved synchronization states are ordinary results.

`tempfile` provides isolated workspaces for filesystem tests.

## Filesystem I/O

Zwirn uses whole-file reads and prepares outputs in memory.

Fragment uniqueness is exact canonical marker-path equality. Distinct marker paths are otherwise independent. Zwirn does not compare fragment destinations with one another or compare filesystem identities.

Direct writes use ordinary platform file creation, truncation, and write semantics, including their effects on destination metadata.

## ADLS source fields

PatchObject pool indexes identify distinct tables. This gives each node handle an independently rewritable object.

Rewriting appends replacement strings and redirects existing `f10` offsets. Audulus accepts this layout and compacts superseded strings when it next saves the document.

## Synchronization state

Classification distinguishes absent-file and matching-file forms of observable `unadopted` state because their safe actions differ. Unequal sources that both match the truncated baseline hash are treated conservatively as a conflict.

Command planning is pure. A forced selection is validated as a complete batch before actions are materialized.

## Command reporting

Results are path-first, tab-separated lines. Mutating commands omit already synchronized fragments and use `record`, `embed`, and `extract` for performed actions. Diagnostics use standard error.
