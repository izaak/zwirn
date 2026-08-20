# Zwirn

Zwirn synchronizes source code fragments in Audulus 4 patches with local files.

One-shot commands are supported on macOS and Linux. macOS also supports
a foreground `live` session.

It is pre-release software intended for trusted, version-controlled
workspaces.

## Install from source

With a recent stable Rust toolchain:

```console
cargo install --locked --path .
```

## Getting started

In an Audulus code inspector, write markers to associate a fragment with a path:

```lua
-- @{ src/consts.lua
local gain = 0.5
-- @} src/consts.lua
```

By default, the path is relative to the Audulus document's parent directory.

Audulus may remain open while Zwirn synchronizes unambiguous
saved changes in both directions:

```console
zwirn sync patch.audulus4
```

If `src/consts.lua` does not exist, `sync` creates it and adds a synchronization
hash to the closing marker:

```lua
-- @} src/consts.lua 9238d3dc5eb11d81
```

The external file contains only the source between the markers. Edit either
copy, save it, then run `sync` again.

## Commands

```text
zwirn <COMMAND>

status   inspect without changes
embed    source files → .audulus4
extract  source files ← .audulus4
sync     source files ↔ .audulus4
live     source files ↔ .audulus4 (foreground; macOS only)
```

One-shot commands accept:

```text
zwirn <status|embed|extract|sync> [OPTIONS] DOCUMENT [FRAGMENT]...
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

## Live synchronization

On macOS, `live` runs in the foreground and watches one document and its source
root, safely synchronizing saved changes in both directions:

```console
zwirn live patch.audulus4
```

## Markers

Fragments begin with an `@{ PATH` marker and end with an `@} PATH` marker.

Marker comments follow the source language: `--` for Lua and `//` for GLSL or
Lyte.

Leave the closing hash out when creating a fragment; Zwirn records it after
synchronization.

A node's source may contain multiple marked fragments.

The [design document](docs/design.md#marker-grammar) defines the complete marker
and path grammar.

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

## One-shot output and exit status

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

## AI assistance

Portions of Zwirn were developed with assistance from AI coding tools,
including OpenAI Codex. Their output was reviewed, tested, and adapted by the
maintainers, who remain responsible for the final implementation.

## License

Zwirn is available under the [MIT License](LICENSE).

Zwirn is an independent project and is not affiliated with or endorsed by
Audulus LLC.
