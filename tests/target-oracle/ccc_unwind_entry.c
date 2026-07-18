#include "abi_types.h"

int ccc_unwind_entry(int depth) {
    volatile int next = depth - 1;
    if (depth > 0) return ccc_unwind_entry(next);
    return target_oracle_unwind_probe(1);
}

int ccc_unwind_variadic(int marker, ...) {
    __builtin_va_list arguments;
    int payload;
    __builtin_va_start(arguments, marker);
    payload = __builtin_va_arg(arguments, int);
    __builtin_va_end(arguments);
    if (payload != 9) return 62;
    return target_oracle_unwind_probe(marker);
}
