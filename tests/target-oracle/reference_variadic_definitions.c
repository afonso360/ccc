#include <stdarg.h>

#include "abi_types.h"

static long collect_copy(va_list incoming) {
    va_list arguments;
    struct Pair pair;
    double floating;
    long total;
    va_copy(arguments, incoming);
    total = va_arg(arguments, int);
    total += va_arg(arguments, int);
    total += (long)va_arg(arguments, double);
    total += va_arg(arguments, long);
    total += va_arg(arguments, long);
    total += va_arg(arguments, long);
    total += va_arg(arguments, long);
    total += va_arg(arguments, long);
    total += va_arg(arguments, long);
    total += va_arg(arguments, long);
    pair = va_arg(arguments, struct Pair);
    floating = va_arg(arguments, double);
    va_end(arguments);
    return total + pair.first + pair.second + (long)floating;
}

long ref_collect(int marker, ...) {
    va_list arguments;
    long first;
    long second;
    va_start(arguments, marker);
    first = collect_copy(arguments);
    second = collect_copy(arguments);
    va_end(arguments);
    return first == second ? marker + first : -1;
}
