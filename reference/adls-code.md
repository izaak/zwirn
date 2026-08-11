# ADLS Code Reference

An `.audulus4` file stores one binary FlatBuffer. Shader, Canvas, DSP, and Lyte
DSP patch objects store embedded source strings.

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

Root field `f0` is the `[PatchObject]` vector. It is always present. Every
supported source string is reached through this vector.

Vector indexes are per-file identities and may change when Audulus saves a
document. Patch object index `0` is the root module.

## Embedded Source

PatchObject field `f0` is the `uint` node type. An absent value decodes as
Module (`0`). The source-bearing node types are:

| Type ID | Node type | Language |
|---:|---|---|
| `74` | Shader | GLSL |
| `78` | Canvas | Lua |
| `79` | DSP | Lua |
| `82` | Lyte DSP | Lyte |

PatchObject field `f10` holds source for all four types and text content for
Text objects. An absent `f10` represents the empty string. Its presence alone
does not identify a supported source string; `f0` determines the node type and
language.

## Rewriting

Rewriters preserve every root field, patch object field, connection, vector,
string, and unknown serialized field other than the source strings they
intentionally replace. The resulting bytes remain a valid FlatBuffer with the
`ADLS` file identifier.
