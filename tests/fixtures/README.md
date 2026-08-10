# Fixtures

These Audulus-authored documents are immutable reference files for the `.audulus4` format. Tests perform mutations on temporary copies.

## `empty.audulus4`

An otherwise empty document with no DSP or Lyte DSP nodes.

## `source-types.audulus4`

A document containing populated and empty Lua DSP and Lyte DSP nodes. Text, Shader, and Canvas nodes provide distinctive non-DSP contents in their shared source field.

## `representative.audulus4`

A working patch with modules, connections, ordinary metadata, marked Lua and Lyte DSP nodes, and an unmarked Lua DSP node. The marked fragments correspond to `angular_smoother.lua` and `angular_smoother.lyte` beside the document.
