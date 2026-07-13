# Execution tests

The harness invokes the built `ccc` binary against every fixture in this
directory. Every host checks the emitted x86-64 ELF objects structurally.
x86-64 Linux additionally links and executes the scalar programs and verifies
their exit status. The empty translation unit is object-only because a normal
hosted C runtime requires `main` when linking an executable.
