#include "x86_f80_abi.h"

long double ref_f80_fixed(
    long prefix,
    double scalar,
    long double precise,
    int tail,
    long double guard
) {
    if (prefix != 7 || scalar != 0.5 || tail != -3 || guard != 2.0L) {
        return -guard;
    }
    return precise;
}

struct F80Box ref_f80_box(
    long prefix,
    struct F80Box box,
    long double delta,
    int suffix
) {
    box.value = box.value + delta + (long double)prefix + (long double)suffix;
    box.tag = box.tag + (unsigned long)(prefix + suffix);
    return box;
}

long double ref_f80_apply(F80Unary function, long double value) {
    return function(value);
}

long double ref_f80_variadic(long double bias, int count, ...) {
    __builtin_va_list arguments;
    long double result = bias;
    __builtin_va_start(arguments, count);
    while (count > 0) {
        result = result + __builtin_va_arg(arguments, long double);
        count = count - 1;
    }
    __builtin_va_end(arguments);
    return result;
}

long double ref_f80_arithmetic(long double value) {
    return -((((value + 2.0L) - 1.0L) * 4.0L) / 2.0L);
}

int ref_f80_relations(long double left, long double right) {
    if (!(left < right)) return 1;
    if (!(left <= right)) return 2;
    if (left > right) return 3;
    if (left >= right) return 4;
    if (left == right) return 5;
    if (!(left != right)) return 6;
    if (!left || !right) return 7;
    return 0;
}

unsigned ref_f80_comparison_mask(long double left, long double right) {
    unsigned result = 0;
    if (left < right) result = result | 1;
    if (left <= right) result = result | 2;
    if (left > right) result = result | 4;
    if (left >= right) result = result | 8;
    if (left == right) result = result | 16;
    if (left != right) result = result | 32;
    if (left) result = result | 64;
    if (!left) result = result | 128;
    return result;
}

long ref_f80_to_signed(long double value) {
    return (long)value;
}

unsigned long ref_f80_unsigned_roundtrip(unsigned long value) {
    return (unsigned long)(long double)value;
}

__int128 ref_f80_signed128_roundtrip(__int128 value) {
    return (__int128)(long double)value;
}

unsigned __int128 ref_f80_unsigned128_roundtrip(unsigned __int128 value) {
    return (unsigned __int128)(long double)value;
}

double ref_f80_to_double(long double value) {
    return (double)value;
}

long double ref_f80_from_float(float value) {
    return (long double)value;
}

long double ref_f80_volatile_roundtrip(
    volatile long double *storage,
    long double value
) {
    *storage = value;
    return *storage;
}
