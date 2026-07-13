# CCC

CCC is the Cranelift C Compiler. The current implementation provides the Rust
workspace, source spans and diagnostics, pp-token dumps, and empty
translation-unit object emission.

```sh
cargo run -p ccc-driver -- -c empty.c
cargo run -p ccc-driver -- --dump-tokens trivial.c
```

`ccc -c` currently accepts only an empty translation unit (including comments
and whitespace). Parsing and code generation for C declarations and functions
are not implemented yet.
