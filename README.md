# CCC

CCC is the Cranelift C Compiler. The current implementation compiles a scalar C
subset to x86-64 Linux ELF objects through a typed CFG and Cranelift. It
supports `int` functions and parameters, arithmetic and comparisons, local
variables, `if`/`else`, `while`, `return`, and direct function calls.

The C11 preprocessing pipeline supports object-like, function-like, and
variadic macros; stringization and token pasting; conditional directives;
quoted, system, computed, and next-header includes; forced inputs; line
control; feature predicates; common GCC pragmas; reproducible predefined
macros; and Make dependency output. Source files are UTF-8 and may begin with a
byte-order mark. The default language mode is `gnu11`; `-std=c11` selects the
strict preprocessing rules.

Integer constants consumed by the current parser use signed `int` semantics;
unsigned and `long` suffixes and values requiring those types are reported as
unsupported after preprocessing.

```sh
cargo run -p ccc-driver -- -c program.c
cargo run -p ccc-driver -- -E -P -I include program.c
cargo run -p ccc-driver -- -MMD -MF program.d -c program.c
cargo run -p ccc-driver -- --dump-pp-tokens program.c
cargo run -p ccc-driver -- --dump-ast program.c
cargo run -p ccc-driver -- --dump-ir program.c
cargo run -p ccc-driver -- --emit=clif program.c
```

On an x86-64 Linux GNU host, the driver discovers and fingerprints the matching
system compiler, include tree, sysroot, and linker. Omitting `-c` then links a
native executable through that target-checked driver. Other hosts can emit and
inspect x86-64 Linux objects and can preprocess against explicit include trees
with `-nostdinc`, `-I`, and `-isystem`.

Additional C types, globals, pointers, arrays, and indirect calls are outside
the parser and semantic subset and are reported rather than approximated.
