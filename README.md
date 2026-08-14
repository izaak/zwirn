# Zwirn

Zwirn synchronizes Lua, Lyte, and GLSL fragments between Audulus 4 documents and
ordinary source files. Embedded source remains editable in Audulus while also
available to other editors and version control. Zwirn transfers unambiguous
changes in either direction.

Zwirn supports macOS and Linux. It is pre-release software intended for stable,
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

Zwirn synchronizes files on disk. Save open source files and close the Audulus
document before running `embed`, `extract`, or `sync`:

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
zwirn <COMMAND> [OPTIONS] <DOCUMENT> [FRAGMENT]...

status   inspect without changes
embed    source files → .audulus4
extract  source files ← .audulus4
sync     source files ↔ .audulus4
```

Each `FRAGMENT` argument is an exact marker path relative to the source root.
Pass one or more to select an exact subset; omit them to select every fragment.

```console
zwirn embed patch.audulus4 src/filter.lua
```

The source root defaults to the document's parent directory. Set an explicit
root with `--source-root`:

```console
zwirn sync --source-root sources patch.audulus4
```

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
`PATH<TAB>RESULT`.

```text
0   every selected fragment is synchronized
1   one or more fragments require attention
2   validation or operational failure
```

After validating and preparing all outputs, Zwirn writes fragment files in path
order and the document last. An operational failure can leave earlier writes in
place.

## Reference

- [Design](docs/design.md) defines observable behavior.
- [Implementation notes](docs/implementation.md) record internal decisions.
- The [ADLS source-field reference](reference/adls-code.md) describes the
  relevant part of the `.audulus4` representation.

## License

Zwirn is available under the [MIT License](LICENSE).
