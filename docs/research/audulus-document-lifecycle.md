# Audulus hot-reload feasibility

Audulus 4.7 on macOS is a SwiftUI `DocumentGroup`/`FileDocument` application backed by AppKit’s `NSDocument` machinery. An open `.audulus4` file is parsed into an in-memory model; ordinary external writes do not automatically replace that model.

Zwirn currently updates documents with a direct `fs::write`. Audulus eventually notices that the file changed, but normally only exposes this through its save-conflict workflow. Choosing Revert reloads Zwirn’s changes.

The key discovery is that writing through `NSFileCoordinator` changes this behavior. Our prototype performs the same byte write under a plain coordinated write claim. Audulus immediately reloads the open document, and updated Lua appears in place while inspecting the associated DSP node. No window cycling, dialogs, UI automation, or app restart is required.

Audulus treats the reloaded document as clean. Instrumentation showed no subsequent hash, size, or inode change, and its Save command was disabled. It therefore does not immediately reserialize or compact Zwirn’s append-only FlatBuffer rewrite. Repeated later-save experiments show that Audulus compacts the rewrite when it next performs a normal save following an in-app edit.

The implementation focus should be:

- Replace Zwirn’s direct document write with an `NSFileCoordinator`-coordinated write on macOS.
- Coordinate the actual prepared output, rather than performing the current write followed by an identical helper rewrite.
- Use ordinary writing options (`[]`). `.forMerging` only asks presenters to save unsaved changes, while replacement/deletion options invoke different lifecycle behavior.
- Preserve Zwirn’s existing synchronization semantics and treat Audulus compaction as a separate, optional concern.

Experiments with `.forReplacing` and `.forMerging` found no benefit over `[]`; neither is an implementation candidate.

The central product result is strong: Zwirn can provide immediate, seamless hot reload of source changes in an already-open Audulus document.
