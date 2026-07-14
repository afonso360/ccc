# CCC resource directory

This directory contains versioned compiler-owned and target-derived headers,
hosted wrappers, and, when required, target runtime shims. The driver validates
`manifest.toml` before adding the resource include directory to the configured
include search.

Only headers whose declarations or macro contracts can be described truthfully
by the effective compilation configuration belong here. Platform ABI headers
remain owned by the resolved target libc and use wrappers only when CCC must
supply compiler builtins.

Every header belongs to exactly one manifest class:

- `compiler_owned` contains target-independent language spelling headers;
- `target_derived` contains declarations or macros computed from the effective
  target configuration; and
- `hosted_wrappers` augments an authoritative libc header and delegates to it
  with `#include_next`.

The validator rejects duplicate classification, unlisted files, missing files,
and paths that appear in more than one class. Reclassifying a header is a
contract change because it changes which component owns its ABI facts.

`stddef.h` is target-derived. `size_t`, `ptrdiff_t`, and `wchar_t` use the
corresponding predefined target type spellings; `max_align_t` derives from the
target's fundamental scalar alignments; and `offsetof` expands to the shared
frontend builtin. The template honors the conventional `__need_*` partial
include protocol so libc wrappers can request one definition without blocking
a later complete include.

The manifest also records the GNU compatibility profile used to select and
parse hosted-header paths. Its checked capability and declined-feature sets
keep the advertised compiler version tied to preprocessing and declaration
syntax that the frontend actually implements. Semantic or code-generation
support is not implied by this profile.
