# Zwirn

`zwirn` is a command-line tool for keeping source fragments on the filesystem synchronized with monolithic Lua, Lyte, and GLSL source embedded in `.audulus4` files.

Large source-bearing nodes can be composed from smaller files that remain convenient to edit, organize, and version in a source repository. The embedded source remains directly editable inside Audulus, and changes can flow in either direction.

Zwirn supports macOS and Linux.

## Documents and source roots

Each invocation operates on one explicitly named `.audulus4` document and one source root.

The source root defaults to the parent of the document path as named. An explicit `--source-root` may be absolute or relative. The document path and an explicit relative source root are resolved from the current working directory.

The source root must exist and be a directory.

Opening the source root establishes the directory that anchors fragment access for the command. Fragment path resolution, parent-directory creation, and external writes remain beneath that directory. Relative symbolic links may be followed when their resolution remains beneath the opened root; absolute or escaping symbolic links are invalid.

Filesystem behavior is defined for a trusted local workspace that remains stable during an invocation. The contents read during discovery define the command's inputs.

## Saved state and coordinated access

On macOS, `status`, `embed`, `extract`, and `sync` coordinate their actual document and fragment reads and writes with filesystem presenters. Linux continues to use direct filesystem access.

The configured named paths remain fixed during a command. If coordination supplies an accessor path different from the named path, the access fails before its filesystem operation runs. Zwirn does not follow the changed path or retry the operation with direct access.

Coordination does not change Zwirn's synchronization states, validation before writes, source-root containment, exclusive creation, write ordering, partial-commit behavior, or reporting. It governs access to saved filesystem contents; Zwirn does not inspect or promise to protect unsaved state held inside Audulus or a source editor. Applications may remain open, but users should normally edit one representation at a time and save changes for them to participate in synchronization.

## Fragments

Fragments are regions of embedded source identified by Zwirn markers. Each fragment corresponds to one source file. Its marker path is its identity.

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

Authors seed fragments by placing unadopted markers in the appropriate source inside Audulus.

### Marker grammar

A marker occupies an entire line, with optional leading indentation and trailing horizontal whitespace. The containing node determines its language and comment syntax:

| Node type | Language | Comment |
|---|---|---|
| Canvas, DSP | Lua | `--` |
| Shader | GLSL | `//` |
| Lyte DSP | Lyte | `//` |

Horizontal whitespace separates the marker tokens, and paths contain no whitespace.

The path on a closing marker exactly matches the canonical path on its opening marker. A hash is the first 16 lowercase hexadecimal digits of the SHA-256 digest and appears only on a closing marker.

Fragments may appear sequentially within a node. They do not nest or overlap. Orphaned, mismatched, and unterminated markers are structural errors.

Fragment source is the complete sequence of lines strictly between its marker lines. Adjacent opening and closing markers represent empty source.

## Paths

Marker paths are nonempty, use `/` separators, and have canonical relative form. A canonical path has no leading slash, backslash, empty segment, `.` or `..` segment, or trailing slash.

A canonical fragment path is resolved relative to the source root. No canonical fragment path is a strict component ancestor of another. Apart from strict ancestry and shared filesystem-file identity, distinct canonical fragment paths are independent. A fragment target lexically equal to the resolved document path is invalid. The document and existing fragment targets identify distinct filesystem files. A successful mutating command does not create a fragment target that identifies the same filesystem file as another fragment target. An existing target is a regular file. File creation may create missing parent directories.

## Discovery

Zwirn scans the source contents of every Canvas, DSP, Shader, and Lyte DSP node in the document. Marker comments are recognized according to the language of the containing node.

The discovered markers define the fragment inventory. Marker paths are unique within the document.

Audulus node identity, graph position, hierarchy, and other node metadata are not part of fragment identity. A source-bearing node may move or receive a new Audulus identity while its marked fragments remain the same.

## Canonical source

Fragment source is `UTF-8` without a byte order mark. Its canonical representation converts `CRLF` and lone `CR` line endings to `LF` and appends an `LF` to nonempty source that lacks one. Empty source remains empty. All other Unicode code points and whitespace participate unchanged.

Classification, hashing, and transferred source use the canonical representation.

The stored hash is the 64-bit prefix of the `SHA-256` digest of canonical fragment source encoded as `UTF-8`. Marker lines are excluded. The hash represents the last source contents known to be synchronized between the filesystem and the document.

## Synchronization states

Let `F` be canonical filesystem source, `E` canonical embedded source, and `H` the stored baseline hash. `F = E` compares canonical source. `F = H` means that the SHA-256 hash of `F` equals `H`, and likewise for `E = H`.

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

A successful action records the hash of the synchronized canonical source in the closing marker.

## Commands

`zwirn status` is read-only and reports the state of every selected fragment.

`zwirn embed` applies safe filesystem-to-embedded actions.

`zwirn extract` applies safe embedded-to-filesystem actions.

`zwirn sync` applies safe actions in both directions and adopts unambiguous fragments.

With no fragment arguments, a command selects every discovered fragment. One or more canonical fragment paths select an exact subset. An unknown selected path is a command error.

`--force` is available on `embed` and `extract` with explicitly selected paths. Every selected fragment must be in `conflict` or `unadopted conflict`. Forced `embed` selects filesystem source. Forced `extract` selects embedded source.

Synchronization commands create and replace source content. Deletion is outside their operation.

## Validation

Discovery and validation complete before writes begin. Validation covers document structure, marker structure, hashes, source encoding, source-root validity, canonical fragment paths, existing fragment-target types, managed-input identity, fragment uniqueness and component ancestry, and command selectors.

A validation failure aborts the command before writing. Selected fragments with ordinary unresolved states are processed independently, allowing safe actions to proceed alongside conflicts, missing files, and states belonging to the opposite direction.

## Writes

All outputs are prepared before writing. External files are written directly in canonical fragment-path order, followed by the document. A fragment target absent during discovery is created exclusively, and its write fails if a filesystem entry occupies that path. After creating it, Zwirn checks whether any other fragment target that was absent during discovery identifies the new file. An alias is an operational failure after the new fragment has been written. Targets present during discovery use ordinary create-or-truncate behavior. Writes stop at the first operational failure. The current destination may be partially written; completed writes and created directories remain in place.

The document mutation set consists of fragment source and closing-marker hashes. Within source strings, all other text is preserved exactly. All other logical document data remains unchanged.

Updating an existing hash replaces only its token. Establishing a hash inserts one separating space and the hash after the path on the closing marker. Existing marker indentation, comment spacing, token spacing, and trailing whitespace are preserved.

The prepared document passes ADLS and marker parsing before writing. A command producing no document change leaves the document file untouched.

## Reporting

Results are ordered by canonical fragment path and written one per line as `PATH<TAB>RESULT`.

`status` results are synchronization states. Mutating-command results are performed actions (`record`, `embed`, or `extract`) and unresolved states.

Exit code `0` means every selected fragment is synchronized after the command.

Exit code `1` means the command completed with one or more selected fragments still requiring attention.

Exit code `2` means a validation or operational failure prevented normal completion.
