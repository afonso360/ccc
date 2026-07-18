#include "abi_types.h"

typedef __builtin_va_list va_list;

static long collect_copy(va_list incoming) {
    va_list arguments;
    struct Pair pair;
    double floating;
    long total;
    __builtin_va_copy(arguments, incoming);
    total = __builtin_va_arg(arguments, int);
    total += __builtin_va_arg(arguments, int);
    total += (long)__builtin_va_arg(arguments, double);
    total += __builtin_va_arg(arguments, long);
    total += __builtin_va_arg(arguments, long);
    total += __builtin_va_arg(arguments, long);
    total += __builtin_va_arg(arguments, long);
    total += __builtin_va_arg(arguments, long);
    total += __builtin_va_arg(arguments, long);
    total += __builtin_va_arg(arguments, long);
    pair = __builtin_va_arg(arguments, struct Pair);
    floating = __builtin_va_arg(arguments, double);
    __builtin_va_end(arguments);
    return total + pair.first + pair.second + (long)floating;
}

long ccc_collect(int marker, ...) {
    va_list arguments;
    long first;
    long second;
    __builtin_va_start(arguments, marker);
    first = collect_copy(arguments);
    second = collect_copy(arguments);
    __builtin_va_end(arguments);
    return first == second ? marker + first : -1;
}
