# Zwirn

Zwirn synchronizes Lua, Lyte, and GLSL fragments between Audulus 4 documents and
ordinary source files. Embedded source remains editable in Audulus while also
available to other editors and version control. Zwirn transfers unambiguous
changes in either direction.

Zwirn supports one-shot synchronization on macOS and Linux. macOS also supports
a foreground `live` session. It is pre-release software intended for stable,
trusted, version-controlled workspaces.

## Install from source

With a recent stable Rust toolchain:

```console
cargo install --locked --path .
```

## Getting started

Mark a region of source inside Audulus with a fragment path:

```lua
-- @{ src/filter.lua
local gain = 0.5
-- @} src/filter.lua
```

Zwirn synchronizes saved files on disk. Save changes in Audulus and in open
source files before running a one-shot command; Audulus and source editors may
remain open:

```console
zwirn sync patch.audulus4
```

If `src/filter.lua` does not exist, `sync` creates it beneath the document's
directory and adds a synchronization hash to the closing marker:

```lua
-- @} src/filter.lua 9238d3dc5eb11d81
```

The external file contains only the source between the markers. Edit either
copy, then run `sync` again.

## Commands

```text
zwirn <COMMAND>

status   inspect without changes
embed    source files → .audulus4
extract  source files ← .audulus4
sync     source files ↔ .audulus4
live     reconcile saved changes in the foreground (macOS only)
```

The one-shot commands accept:

```text
zwirn <status|embed|extract|sync> [--source-root DIR] DOCUMENT [FRAGMENT]...
```

Each `FRAGMENT` is an exact marker path relative to the source root. Pass one or
more to select an exact subset; omit them to select every fragment.

```console
zwirn embed patch.audulus4 src/filter.lua
```

The source root defaults to the document's parent directory. Set an explicit
root with `--source-root`:

```console
zwirn sync --source-root sources patch.audulus4
```

## Live synchronization on macOS

`live` starts a foreground session for one document and its complete fragment
inventory:

```text
zwirn live [--source-root DIR] DOCUMENT
```

For example:

```console
zwirn live --source-root sources patch.audulus4
```

The session starts filesystem monitoring before an immediate safe,
bidirectional synchronization. It then reconciles newly saved changes under the
source root or at the document path. Live mode uses the same conflict,
containment, coordinated-access, validation, and ordered-write behavior as
`sync`; it neither forces conflicts nor recreates an established fragment whose
external file is missing.

Reconciliation failures are reported without ending an operational session. A
later filesystem change requests another reconciliation, so correcting and
saving a blocked document or fragment can recover the session. There is no
timed retry when nothing changes. Routine reconciliations with no action or
reported-state change remain quiet.

Press Control-C or send `SIGTERM` to request an orderly shutdown. An active
reconciliation finishes before the process exits successfully. Live mode is
visible but unsupported on Linux, where invoking it reports an error and exits
with status 2.

## Markers

Marker syntax follows the source-bearing node type:

| Node type   | Language | Marker comment |
|-------------|----------|----------------|
| Canvas, DSP | Lua      | `--`           |
| Shader      | GLSL     | `//`           |
| Lyte DSP    | Lyte     | `//`           |

A source node may contain multiple sequential fragments.

## Conflicts

Once synchronized, a closing marker's 16-character SHA-256 prefix records the
last synchronized source. Zwirn transfers one-sided changes; divergent changes
remain unresolved.

Resolve a conflict manually, or explicitly select which side wins:

```console
zwirn embed --force patch.audulus4 src/filter.lua
zwirn extract --force patch.audulus4 src/filter.lua
```

With explicitly selected conflicts, forced `embed` selects the filesystem copy;
forced `extract` selects the embedded copy.

## Output and exit status

Results are ordered by fragment path and written one per line as
`PATH<TAB>RESULT` for one-shot commands. Live mode writes human-oriented session
diagnostics to standard error instead of defining a machine-readable output
format.

```text
0   every selected fragment is synchronized
1   one or more fragments require attention
2   validation or operational failure
```

After validating and preparing all outputs, Zwirn writes fragment files in path
order and the document last. An operational failure can leave earlier writes in
place. In live mode, reconciliation attention and recoverable blockers do not
become the foreground process's exit status; orderly signal shutdown exits 0,
while session startup failure exits 2.

## Reference

- [Design](docs/design.md) defines observable behavior.
- [Implementation notes](docs/implementation.md) record internal decisions.
- The [ADLS source-field reference](reference/adls-code.md) describes the
  relevant part of the `.audulus4` representation.

## AI assistance

Portions of Zwirn were developed with assistance from AI coding tools,
including OpenAI Codex. Their output was reviewed, tested, and adapted by the
maintainers, who remain responsible for the final implementation.

## License

Zwirn is available under the [MIT License](LICENSE).

Zwirn is an independent project and is not affiliated with or endorsed by
Audulus LLC.
