# Hosted-header compatibility fixture

The `glibc-like` tree is original project test data. It is not copied from the
GNU C Library. It models public preprocessing patterns used by hosted GNU
headers: include guards, compiler-version gates, feature predicates, computed
macros, GNU attributes, diagnostic pragmas, and nested system includes.

The fixture is available under the repository's Apache-2.0 OR MIT license. Its
shape is pinned in the repository so deterministic preprocessing output can be
reviewed independently from the mutable libc installed on a CI runner.
`manifest.toml` records its origin, license, revision, entry point, compatibility
profile, and complete file inventory.
