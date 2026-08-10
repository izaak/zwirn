# ADLS Code Reference

An `.audulus4` file stores one binary FlatBuffer. DSP and Lyte DSP patch objects
store embedded source strings.

## Header

```text
00..03  root table uoffset_t
04..07  file identifier "ADLS"
```

Readers follow FlatBuffers offsets and vtables from the file. They do not rely
on observed root offsets or baseline physical layouts.

A table field is present when its vtable entry is present and nonzero. An absent
scalar field has its FlatBuffers default value.

## Patch Object Pool

Root field `f0` is the `[PatchObject]` vector. It is always present. Every DSP
source string is reached through this vector.

Vector indexes are per-file identities and may change when Audulus saves a
document. Patch object index `0` is the root module.

## DSP Source

PatchObject field `f0` is the `uint` node type. An absent value decodes as
Module (`0`). The source-bearing node types are:

| Type ID | Node type | Language |
|---:|---|---|
| `79` | DSP | Lua |
| `82` | Lyte DSP | Lyte |

For both types, PatchObject field `f10` is the source `string` and is always
present in Audulus-authored output.

PatchObject field `f10` is also used by Text, Shader, and Canvas objects. Its
presence alone does not identify a DSP source string; `f0` determines how it is
interpreted.

## Rewriting

Rewriters preserve every root field, patch object field, connection, vector,
string, and unknown serialized field other than the DSP source strings they
intentionally replace. The resulting bytes remain a valid FlatBuffer with the
`ADLS` file identifier.
