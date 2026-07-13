# CCC

CCC is the Cranelift C Compiler. The current implementation compiles a scalar C
subset to x86-64 Linux ELF objects through a typed CFG and Cranelift. It
supports `int` functions and parameters, arithmetic and comparisons, local
variables, `if`/`else`, `while`, `return`, and direct function calls.
Source files must be UTF-8. Integer constants currently use signed `int`
semantics; unsigned and `long` suffixes and values requiring those types are
reported as unsupported.

```sh
cargo run -p ccc-driver -- -c program.c
cargo run -p ccc-driver -- --dump-ast program.c
cargo run -p ccc-driver -- --dump-ir program.c
cargo run -p ccc-driver -- --emit=clif program.c
```

On an x86-64 Linux GNU host, omitting `-c` links a native executable through a
target-checked system compiler driver. Other hosts can still emit and inspect
x86-64 Linux objects. Preprocessor directives, additional C types, globals,
pointers, arrays, and indirect calls are outside the supported subset and are
reported as syntax or semantic errors. Preprocessing translation phases,
including backslash-newline splicing, are not implemented yet.
