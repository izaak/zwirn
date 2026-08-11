# Implementation

This document records implementation decisions as Zwirn develops. `docs/design.md` remains normative for observable behavior.

## Libraries

`clap` parses the command line. The command-line layer translates derived values into ordinary Zwirn values before calling library code, whose shape follows the domain independently of the parser.

`sha2` provides SHA-256. Hash parsing, formatting, and truncation produce a Zwirn-owned baseline-hash value, while SHA-2 digest types remain within the hashing implementation.

`thiserror` derives typed validation and operational errors. Unresolved synchronization states are ordinary results.

`flatbuffers` verifies the narrow ADLS read view. Zwirn defines private views for root `f0` and PatchObject `f0`/`f10`, validates reached table spans, and owns preservation-oriented rewriting. The dependency is pinned exactly while its Rust API remains experimental.

## Synchronization state

The synchronization-state representation is deferred until the surrounding inputs, outputs, and orchestration boundaries are established.
