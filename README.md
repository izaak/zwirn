# Zwirn

Zwirn synchronizes source fragments between ordinary files and the Lua, Lyte,
and GLSL embedded in Audulus 4 documents.

Marked source remains editable inside Audulus while also living in files that
are convenient to organize, edit, and version. Zwirn detects which side changed
and transfers source when the direction is unambiguous.

Zwirn supports macOS and Linux. It is pre-release software intended for stable,
trusted, version-controlled workspaces.

## Install from source

A recent stable Rust toolchain is required. From a source checkout:

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

Save the document, then synchronize it:

```console
zwirn sync patch.audulus4
```

With the default source root and no existing target, Zwirn creates
`src/filter.lua` beneath the document's directory and records a synchronization
hash on the closing marker:

```lua
-- @} src/filter.lua 9238d3dc5eb11d81
```

The external file contains only the source between the markers. Zwirn maintains
the hash as the shared baseline, so subsequent changes can flow from the file
into Audulus or from a saved Audulus document back into the file:

```console
zwirn status patch.audulus4
zwirn sync patch.audulus4
```

## Markers

Zwirn scans source-bearing Canvas, DSP, Shader, and Lyte DSP nodes.

| Node type   | Language | Marker comment |
|-------------|----------|----------------|
| Canvas, DSP | Lua      | `--`           |
| Shader      | GLSL     | `//`           |
| Lyte DSP    | Lyte     | `//`           |

For example, a GLSL fragment uses `//` markers:

```glsl
// @{ shaders/color.glsl
vec3 color = vec3(1.0);
// @} shaders/color.glsl
```

A source node may contain multiple sequential fragments. Each marker path
identifies one fragment relative to the source root, which defaults to the
document's parent directory.

## Commands

```text
zwirn <COMMAND> [OPTIONS] <DOCUMENT> [FRAGMENT]...

status   inspect without changes
embed    source files → .audulus4
extract  source files ← .audulus4
sync     source files ↔ .audulus4
```

Omitting fragment arguments selects every fragment. Passing paths selects an
exact subset:

```console
zwirn embed patch.audulus4 src/filter.lua
```

Set an explicit source root with `--source-root`:

```console
zwirn sync --source-root sources patch.audulus4
```

## Conflicts

Each adopted closing marker stores a 16-character prefix of the SHA-256 hash
from the last synchronized source. Zwirn compares that baseline with the
embedded and filesystem copies. A one-sided change has a safe direction;
divergent changes on both sides remain unresolved.

Resolve a conflict manually, or explicitly select which side wins:

```console
zwirn embed --force patch.audulus4 src/filter.lua
zwirn extract --force patch.audulus4 src/filter.lua
```

Forced `embed` selects the filesystem copy. Forced `extract` selects the
embedded copy. Force applies only to explicitly selected conflicting fragments.

## Output and exit status

Results are ordered by fragment path and written one per line as
`PATH<TAB>RESULT`.

- Exit status `0` means every selected fragment is synchronized.
- Exit status `1` means the command completed with one or more fragments still
  requiring attention.
- Exit status `2` means a validation or operational failure prevented normal
  completion.

Zwirn validates and prepares all outputs before writing. It writes fragment
files directly in path order, followed by the document. An operational failure
can leave completed writes in place.

## Reference

[Design](docs/design.md) defines observable behavior. [Implementation
notes](docs/implementation.md) record internal decisions, and [ADLS source
fields](reference/adls-code.md) describes the relevant part of the `.audulus4`
representation.

## License

Zwirn is available under the [MIT License](LICENSE).
