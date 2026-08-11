# Fixtures

These Audulus-authored documents are immutable reference files for the `.audulus4` format. Tests perform mutations on temporary copies.

## `empty.audulus4`

An otherwise empty document with no source-bearing nodes.

## `source-types.audulus4`

A document containing populated and empty DSP and Lyte DSP nodes, plus populated Shader and Canvas nodes. A Text node provides distinctive non-source contents in the shared `f10` field.

## `representative.audulus4`

A working patch with modules, connections, ordinary metadata, marked DSP and Lyte DSP nodes, and unmarked DSP and Canvas nodes. The marked fragments correspond to `angular_smoother.lua` and `angular_smoother.lyte` beside the document.
