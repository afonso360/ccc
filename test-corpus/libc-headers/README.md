# Hosted-header compatibility fixture

The `glibc-like` tree is original project test data. It is not copied from the
GNU C Library. It models public preprocessing patterns used by hosted GNU
headers: include guards, compiler-version gates, feature predicates, computed
macros, GNU attributes, diagnostic pragmas, nested system includes, alternative
keyword spellings, `typeof`, restricted pointers, and declaration assembly
labels.

The fixture is available under the repository's Apache-2.0 OR MIT license. Its
shape is pinned in the repository so deterministic preprocessing output can be
reviewed independently from the mutable libc installed on a CI runner.
`manifest.toml` records its origin, license, revision, entry point, compatibility
profile, modeled parser requirements, and complete file inventory. The fixture
certifies preprocessing, token conversion, and parsing. Its declaration surface
is not a claim that hosted headers may proceed into semantic analysis or code
generation.
