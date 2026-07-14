# x86-64 ABI layout oracle

The driver integration test compiles `layout_objects.c` with CCC, GCC, and
Clang, verifies that every compiler targets x86-64 Linux GNU ELF, and compares
the resulting named objects. Both independent compilers are required; a
missing or mis-targeted compiler fails the test.

The scalar table compares size, alignment, and addressable member offsets.
Bit-field samples are compared as the XOR between a zero baseline and a
one-hot or maximum-value object from the same compiler. This removes
unspecified padding bytes from the comparison while retaining every bit whose
placement is ABI-significant. The cases cover mixed declared types, positive
and negative signed values, zero-width barriers, allocation-unit boundaries,
unnamed fields, nested records, unions, and packed records.

The test is compiled only on x86-64 Linux. `CCC_ABI_GCC` and `CCC_ABI_CLANG`
can override the required driver commands, including command-line target
options. Their defaults are `gcc` and `clang` respectively.
