# Zwirn

`zwirn` is a command-line tool for keeping source fragments on the filesystem synchronized with monolithic Lua and Lyte DSP source embedded in `.audulus4` files.

Large DSP nodes can be composed from smaller files that remain convenient to edit, organize, and version in a source repository. The embedded source remains directly editable inside Audulus, and changes can flow in either direction.

## Fragments

Fragments are regions of embedded DSP source identified by Zwirn markers.

```text
<comment> @{ ID PATH
SOURCE
<comment> @} ID [HASH]
```

For Lua:

```lua
-- @{ svf src/filter/svf.lua
...source...
-- @} svf sha256:f5d2...
```

For Lyte:

```text
// @{ svf src/filter/svf.lyte
...source...
// @} svf sha256:f5d2...
```

The fragment `ID` is stable and independent of its filesystem path. IDs are unique within a Zwirn project.

The `PATH` is relative to the project source tree.

The `HASH` represents the last source contents known to be synchronized between the filesystem and the Audulus document.

An absent hash represents an unadopted fragment:

```lua
-- @{ svf src/filter/svf.lua
...source...
-- @} svf
```

Authors seed new fragments by placing these initial markers in the appropriate DSP source inside Audulus.

## Discovery

Zwirn scans the source contents of every Lua and Lyte DSP node in the `.audulus4` document and discovers fragments by their markers.

Audulus node identity, graph position, hierarchy, and other node metadata are not part of fragment identity. A DSP node may move or receive a new Audulus identity without affecting synchronization as long as its marked source contents are preserved.

The fragment marker itself is the identity of the embedded region.

Duplicate fragment IDs are errors.

Multiple fragment IDs referring to the same source path are errors.

## Hashes and synchronization

The stored hash is the common baseline used to determine which copy has changed.

For each fragment:

- filesystem and embedded source match the hash: `synchronized`
- filesystem changed and embedded source matches the hash: `push`
- embedded source changed and filesystem matches the hash: `pull`
- both changed from the hash: `conflict`
- no hash exists: `unadopted`

A successful synchronization updates the embedded marker with the new baseline hash.

Hashes are SHA-256 hashes of the fragment source encoded as UTF-8, with line endings normalized to LF. Marker lines are excluded from the hash. Other whitespace and source formatting are preserved as meaningful content.

## Commands

`zwirn status` reports the state of every discovered fragment.

`zwirn push` writes changed filesystem source into the corresponding embedded fragments.

`zwirn pull` writes changed embedded source back to the corresponding filesystem files.

`zwirn sync` performs every unambiguous push and pull in both directions and establishes baselines for unadopted fragments.

Conflicts are reported and left unchanged.

## Missing fragments and files

Missing source files and fragments missing from the Audulus document are reported.

Zwirn does not infer deletion from absence and does not remove source files or embedded source regions.
