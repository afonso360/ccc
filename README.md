# CCC

CCC is the Cranelift C Compiler. The current implementation compiles a scalar
and aggregate C11 subset to x86-64 Linux ELF objects through a fully typed AST,
CCC-IR control-flow graph, System V ABI plans, and Cranelift. Implemented
behavior includes fundamental integer types, `_Bool`, `float`, `double`,
pointers, arrays, fixed-prototype function pointers, structures, unions,
enumerations, bit-fields, scalar and aggregate initializers, string literals,
globals, block statics, linkage, tentative definitions, direct and indirect
scalar and aggregate calls, prototyped variadic calls and definitions, C
control flow, and `const`, `restrict`, and `volatile` qualifiers. Type widths,
plain-`char` signedness, layout, packing, conversions, and volatile accesses
come from the effective target configuration.

The C11 preprocessing pipeline supports object-like, function-like, and
variadic macros; stringization and token pasting; conditional directives;
quoted, system, computed, and next-header includes; forced inputs; line
control; feature predicates; common GCC pragmas; reproducible predefined
macros; and Make dependency output. Source files are UTF-8 and may begin with a
byte-order mark. The default language mode is `gnu11`; `-std=c11` selects the
strict preprocessing rules.

The hosted GNU compatibility profile is certified through parsing. Its
reserved alternative keywords, declaration attributes, `typeof`, and assembly
labels remain represented in the untyped AST, while actions requiring
parse-only semantics fail before typed output or object emission. Installed
system headers use the discovered target include search; the compiler-owned
resource directory supplies target-derived headers such as `<stddef.h>`.

Unavailable behavior is diagnosed rather than approximated. Current hard
boundaries include native `long double` arithmetic and calls, runtime-sized
automatic arrays, atomic operations, unprototyped calls, and GNU attributes or
assembly labels whose observable semantics are not implemented.

```sh
cargo run -p ccc-driver -- -c program.c
cargo run -p ccc-driver -- -E -P -I include program.c
cargo run -p ccc-driver -- -MMD -MF program.d -c program.c
cargo run -p ccc-driver -- --dump-pp-tokens program.c
cargo run -p ccc-driver -- --dump-ast program.c
cargo run -p ccc-driver -- --dump-typed-ast program.c
cargo run -p ccc-driver -- --dump-ir program.c
cargo run -p ccc-driver -- --dump-abi program.c
cargo run -p ccc-driver -- --emit=clif program.c
```

On an x86-64 Linux GNU host, the driver discovers and fingerprints the matching
system compiler, include tree, sysroot, and linker. Omitting `-c` then links a
native executable through that target-checked driver. Other hosts can emit and
inspect x86-64 Linux objects and can preprocess against explicit include trees
with `-nostdinc`, `-I`, and `-isystem`.
