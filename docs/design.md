# Zwirn

`zwirn` is a command-line tool for keeping source fragments on the filesystem
synchronized with monolithic Lua, Lyte, and GLSL source embedded in `.audulus4`
files.

Large source-bearing nodes can be composed from smaller files that remain
convenient to edit, organize, and version in a source repository. The embedded
source remains directly editable inside Audulus, and changes can flow in either
direction.

Zwirn supports one-shot synchronization on macOS and Linux. macOS also
supports foreground live synchronization.

## Documents and source roots

Each one-shot invocation or live session operates on one explicitly named
`.audulus4` document and one source root.

The document path and an explicit `--source-root` may be absolute or relative.
When a command or session starts, relative paths are resolved from the current
working directory. If `--source-root` is omitted, the source root is the parent
of the resolved document path without canonicalizing that path.

The source root must exist and be a directory.

Opening the source root establishes the directory that anchors fragment access
for the one-shot command or live reconciliation. Fragment target resolution,
reads, parent-directory creation, and writes remain beneath that directory.
Relative symbolic links are permitted only when their resolution remains
beneath the opened root; absolute or escaping symbolic links are invalid.

The filesystem contract assumes a trusted local workspace. Apart from races
described explicitly below, behavior is unspecified if another process changes
the workspace during an individual command or reconciliation. The contents
read during discovery define that operation's inputs.

## Saved state and coordinated access

On macOS, `status`, `embed`, `extract`, `sync`, and each live reconciliation use
coordinated filesystem access so participating applications can mediate
document and fragment reads and writes. Linux one-shot commands use direct
filesystem access.

The configured document and source-root paths remain fixed throughout a
one-shot command or live session. The markers discovered for an individual
command or reconciliation fix its fragment target paths. Zwirn does not follow
a file or directory moved to a different path.

Coordination affects only access to saved filesystem contents; other
synchronization behavior remains unchanged. Zwirn does not inspect or protect
unsaved state held inside Audulus or a source editor. Applications may remain
open, but only saved changes participate in synchronization. Editing one
representation at a time avoids competing saved changes.

## Fragments

Fragments are regions of embedded source identified by Zwirn markers. Each
fragment corresponds to one source file. Its marker path is its identity.

The general marker form is:

```text
<comment> @{ PATH
SOURCE
<comment> @} PATH [HASH]
```

For Lua:

```lua
-- @{ src/filter/svf.lua
...source...
-- @} src/filter/svf.lua 0123456789abcdef
```

For Lyte:

```text
// @{ src/filter/svf.lyte
...source...
// @} src/filter/svf.lyte 0123456789abcdef
```

An absent hash marks an unadopted fragment:

```lua
-- @{ src/filter/svf.lua
...source...
-- @} src/filter/svf.lua
```

Authors seed fragments by placing unadopted markers in the appropriate source
inside Audulus.

### Marker grammar

A marker occupies an entire line, with optional leading indentation and
trailing horizontal whitespace. In the marker grammar, horizontal whitespace
means ASCII space or tab. The containing node determines its language and
comment syntax:

| Node type | Language | Comment |
|---|---|---|
| Canvas, DSP | Lua | `--` |
| Shader | GLSL | `//` |
| Lyte DSP | Lyte | `//` |

Horizontal whitespace separates the marker tokens, and paths contain no whitespace.

The path on a closing marker exactly matches the canonical path on its opening
marker. A hash consists of 16 lowercase hexadecimal digits and appears only on
a closing marker.

Fragments may appear sequentially within a node. They do not nest or overlap.
Orphaned, mismatched, and unterminated markers are structural errors.

Fragment source is the complete sequence of lines strictly between its marker
lines. Adjacent opening and closing markers represent empty source.

## Paths

Marker paths are nonempty, use `/` separators, and have canonical relative
form. A canonical path has no leading slash, backslash, empty segment, `.` or
`..` segment, or trailing slash.

A canonical fragment path is resolved relative to the source root. No fragment
path may be a strict component ancestor of another. Distinct paths are
otherwise treated independently unless their targets identify the same
filesystem file.

A fragment target lexically equal to the resolved document path is invalid.
The document and existing fragment targets must all identify distinct
filesystem files. A mutating command that completes without a validation or
operational failure does not create a fragment target that identifies the same
filesystem file as another fragment target.

An existing fragment target must be a regular file. Creating a target first
creates any missing parent directories.

## Discovery

Zwirn scans the source contents of every Canvas, DSP, Shader, and Lyte DSP node
in the document. Marker comments are recognized according to the language of
the containing node.

The discovered markers define the fragment inventory. Marker paths are unique
within the document.

Audulus node identity, graph position, hierarchy, and other node metadata are
not part of fragment identity. A source-bearing node may move or receive a new
Audulus identity while its marked fragments remain the same.

## Canonical source

Fragment source is `UTF-8` without a byte order mark. Its canonical
representation converts `CRLF` and lone `CR` line endings to `LF`. Nonempty
source receives a final `LF` if it lacks one; empty source remains empty. All
other Unicode code points and whitespace participate unchanged.

Classification, hashing, and transferred source use the canonical representation.

The stored hash is the 64-bit prefix of the `SHA-256` digest of canonical
fragment source encoded as `UTF-8`. Marker lines are excluded. The hash
represents the last source contents known to be synchronized between the
filesystem and the document.

## Synchronization states

Let `F` be canonical filesystem source, `E` canonical embedded source, and `H`
the stored baseline hash. `F = E` compares canonical source directly. `F = H`
means that the stored 64-bit prefix of the `SHA-256` digest of `F` equals `H`,
and likewise for `E = H`.

| `H` | Filesystem file | Relationship | State | Safe action |
|---|---|---|---|---|
| absent | absent | — | `unadopted` | `sync` or `extract` creates `F` and records `H` |
| absent | present | `F = E` | `unadopted` | any mutating command records `H` |
| absent | present | `F ≠ E` | `unadopted conflict` | targeted forced `embed` or `extract` |
| present | absent | — | `missing` | `extract` with an explicit path recreates `F` |
| present | present | `F = E = H` | `synchronized` | none |
| present | present | `E = H`, `F ≠ H` | `embed` | `embed` or `sync` |
| present | present | `F = H`, `E ≠ H` | `extract` | `extract` or `sync` |
| present | present | `F = E`, both differing from `H` | `converged` | any mutating command records the new `H` |
| present | present | `F ≠ E`, `F = H`, `E = H` | `conflict` | manual convergence or targeted forced `embed` or `extract` |
| present | present | `F ≠ E`, `F ≠ H`, `E ≠ H` | `conflict` | manual convergence or targeted forced `embed` or `extract` |

A truncated-hash collision can make unequal `F` and `E` both match `H`. Zwirn
classifies that case as a conflict.

A successful action records the hash of the synchronized canonical source in
the closing marker.

## One-shot commands

`zwirn status` is read-only and reports the state of every selected fragment.

`zwirn embed` applies safe filesystem-to-embedded actions.

`zwirn extract` applies safe embedded-to-filesystem actions.

`zwirn sync` applies safe actions in both directions and adopts unambiguous fragments.

With no fragment arguments, a one-shot command selects every discovered
fragment. One or more canonical fragment paths select an exact subset. An
unknown selected path is a command error.

`--force` is available on `embed` and `extract` with explicitly selected paths.
Every selected fragment must be in `conflict` or `unadopted conflict`. Forced
`embed` selects filesystem source. Forced `extract` selects embedded source.

Mutating one-shot commands create and replace source content. Deletion is
outside their operation.

## Live mode

On macOS, the foreground live command is:

```text
zwirn live [--source-root DIR] DOCUMENT
```

The document and source root follow the same resolution and validation rules
as one-shot commands. Each reconciliation freshly discovers the complete
fragment inventory and applies the same safe, bidirectional behavior as
`zwirn sync`.

Live mode does not add deletion. In particular, it reports a fragment in
`missing` state but does not recreate its external file automatically.

A document replaced at its configured path remains in scope, but live mode does
not follow a document or source root moved to another path.

On Linux, `live` remains visible in the command-line interface but reports that
it is unsupported and exits with status 2.

### Monitoring and reconciliation

Live mode watches the source-root hierarchy and the parent of the configured
document. Monitoring starts before an immediate initial reconciliation, which
discovers the state already present at startup. If the session continues, a
hint delivered before or during that reconciliation remains pending for a
subsequent reconciliation. This leaves no gap between monitoring startup and
the initial run.

Filesystem events are invalidation hints, not an ordered history of changes.
Every hint delivered from either watched scope is relevant, including activity
at paths unrelated to the configured document or discovered fragment targets
and activity caused by Zwirn's own writes. A hint requests a fresh
reconciliation of the document and its full fragment inventory. Hints and
reconciliations do not correspond one-to-one.

Reconciliations are serialized, and multiple hints may be coalesced. A hint
that arrives during a reconciliation guarantees at least one subsequent
reconciliation rather than being absorbed into the active run.

Indications of dropped events or a watched-root change request a full
reconciliation. If filesystem monitoring cannot be established, live mode
reports the failure and exits with status 2.

Once monitoring is operational, unresolved fragment states and validation or
operational failures are nonfatal reconciliation blockers. A blocker does not
terminate the session, which remains responsive to filesystem hints. A later
hint triggers a fresh reconciliation, allowing another attempt after, for
example, a user corrects and saves a malformed fragment. Without another hint,
live mode does not retry on a timer.

### Diagnostics and shutdown

Live diagnostics are written to standard error for the foreground human
workflow, not as a stable machine-readable protocol. They report startup,
shutdown, performed actions, new or changed blockers, and recovery from
blockers. Routine reconciliations that perform no action and do not change the
reported state remain silent. Exact wording and line structure are not part of
the contract.

`SIGINT` and `SIGTERM` request an orderly shutdown. An idle session stops
without waiting for filesystem activity. An active reconciliation finishes
before exit, which may delay shutdown. A signal racing with the start of a
reconciliation may allow that reconciliation to run. Once shutdown is accepted,
no further reconciliation begins, and the process exits with status 0.

Live mode exits with status 2 if startup fails, if Zwirn detects that signal
handling or internal event delivery has stopped, or if it cannot write
diagnostics. Reconciliation blockers do not determine the live process's exit
status.

## Validation

Discovery and validation complete before writes begin. Validation covers:

- document and marker structure;
- hashes and source encoding;
- source-root validity, canonical fragment paths, and existing fragment-target
  types;
- distinct filesystem identities for the document and existing fragment
  targets;
- unique fragment paths and the ban on strict component ancestry; and
- command selectors.

A validation failure aborts a one-shot command or the current live
reconciliation before writing. An unresolved synchronization state is a
per-fragment result rather than a validation failure. Such fragments are
processed independently, allowing safe actions to proceed alongside conflicts
and missing files. For a directional one-shot command, states belonging to the
opposite direction likewise do not prevent other safe actions.

## Writes

All outputs are prepared before writing. External files are written directly in
canonical fragment-path order, followed by the document.

Zwirn opens a fragment target present during discovery with create-or-truncate
semantics. An absent target is created exclusively; its write fails if a
filesystem entry has appeared at that path.

After creating an absent target, Zwirn checks whether it identifies the same
filesystem file as any other fragment target that was absent during discovery.
An alias is an operational failure after the new fragment has been written.

Writes stop at the first operational failure. The current destination may be
partially written; completed writes and created directories remain in place.

Document writes change only fragment source and closing-marker hashes. Within
source strings, all other text is preserved exactly. All other logical document
data remains unchanged.

Updating an existing hash replaces only its token. Establishing a hash inserts
one separating space and the hash after the path on the closing marker.
Existing marker indentation, comment spacing, token spacing, and trailing
whitespace are preserved.

The prepared document is validated before writing. An operation producing no
document change leaves the document file untouched.

## One-shot reporting and exit status

One-shot results are ordered by canonical fragment path and written one per line
as `PATH<TAB>RESULT`.

`status` results are synchronization states. Mutating-command results are
performed actions (`record`, `embed`, or `extract`) and unresolved states.

| Exit code | Meaning |
|---:|---|
| `0` | Every selected fragment is synchronized after the command. |
| `1` | The command completed with one or more selected fragments still requiring attention. |
| `2` | The command ended with a validation or operational failure. |
