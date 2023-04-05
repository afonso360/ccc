# Clan-Cranelift C Compiler (CCC)

A C Compiler based on cranelift using clang as a frontend.

# Compiling

Since the project depends on clang/llvm you may need to install the development libraries. On Ubuntu usually this is sufficient:

```
sudo apt install libclang-dev
```

# Testsuite

## `test: compile`

A comment at the top of the file with `// test: compile` will compile the file.

## `test: run`

A comment at the top of the file with `// test: run` will compile the file and run it. It checks that the return code is 0.