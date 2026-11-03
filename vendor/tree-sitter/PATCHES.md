# Local tree-sitter patch

Upstream crates.io tree-sitter 0.26.11, MIT; LICENSE retained unchanged.
Only src/stack.c changes: StackNode uses one inline link and allocates
MAX_LINK_COUNT links when a second link is required. The limit remains8.
Overflow storage is freed before node recycling/free; nodes retain stable
addresses. No allocator hook or diagnostic global counter is introduced.

Provenance and qualification records are kept outside this package.
The initial integration requires workspace/MSRV/native/GUI qualification.
Recheck this patch against every upstream update; do not silently rebase it.
