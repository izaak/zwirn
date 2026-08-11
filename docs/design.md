# Zwirn

`zwirn` is a command-line tool for keeping source fragments on the filesystem synchronized with monolithic Lua, Lyte, and GLSL source embedded in `.audulus4` files.

Large source-bearing nodes can be composed from smaller files that remain convenient to edit, organize, and version in a source repository. The embedded source remains directly editable inside Audulus, and changes can flow in either direction.

## Documents and source roots

Each invocation operates on one explicitly named `.audulus4` document and one source root.

The source root defaults to the directory containing the document. An explicit `--source-root` may be absolute or relative. The document path and an explicit relative source root are resolved from the current working directory.

The source root must exist and be a directory.

## Fragments

Fragments are regions of embedded source identified by Zwirn markers. Each fragment corresponds to one file beneath the source root. Its source-root-relative path is its identity.

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

A referenced path remains beneath the canonical source root. Its existing components are ordinary directories rather than symbolic links. An existing target is a regular file. File creation may create missing parent directories beneath the source root.

Distinct marker paths identify distinct filesystem entries. Filesystem aliases, including case-folding aliases, are duplicate-path errors.

A fragment rename moves the source file and changes the path on both embedded markers.

## Discovery

Zwirn scans the source contents of every Canvas, DSP, Shader, and Lyte DSP node in the document. Marker comments are recognized according to the language of the containing node.

The discovered markers define the fragment inventory. Fragment paths are unique within the document and source root.

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
| present | absent | — | `missing` | targeted `extract` recreates `F` |
| present | present | `F = E = H` | `synchronized` | none |
| present | present | `E = H`, `F ≠ H` | `embed` | `embed` or `sync` |
| present | present | `F = H`, `E ≠ H` | `extract` | `extract` or `sync` |
| present | present | `F = E`, both differing from `H` | `converged` | any mutating command records the new `H` |
| present | present | `F`, `E`, and `H` all differ | `conflict` | manual convergence or targeted forced `embed` or `extract` |

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

Discovery and validation complete before writes begin. Validation covers the document structure, marker structure, hashes, source encoding, source-root paths, filesystem targets, fragment uniqueness, and command selectors.

A validation failure aborts the command before writing. Selected fragments with ordinary unresolved states are processed independently, allowing safe actions to proceed alongside conflicts, missing files, and states belonging to the opposite direction.

## Writes

Each output is prepared before replacement, and each destination is replaced atomically.

External file replacements precede document replacement. The document is replaced after all planned external writes succeed. A later failure leaves committed external files in place and reports them.

The document mutation set consists of fragment source and closing-marker hashes. Within source strings, all other text is preserved exactly. All other logical document data remains unchanged.

Updating an existing hash replaces only its token. Establishing a hash inserts one separating space and the hash after the path on the closing marker. Existing marker indentation, comment spacing, token spacing, and trailing whitespace are preserved.

The completed document is parsed successfully before atomic replacement and retains the original document's filesystem permissions. A command producing no document change leaves the document file untouched.

Implementations should verify that inputs remain unchanged between discovery and writing, aborting if a change is detected.

## Reporting

Results are ordered by canonical fragment path.

Mutating commands report each action performed and every unresolved state.

Exit code `0` means every selected fragment is synchronized after the command.

Exit code `1` means the command completed with one or more selected fragments still requiring attention.

Exit code `2` means a validation or operational failure prevented normal completion.
