#include <stdarg.h>

enum Choice {
    CHOICE = 11
};

struct Bits {
    unsigned narrow : 5;
    unsigned wide : 32;
};

static int sum_twice(int count, ...) {
    va_list first;
    va_list second;
    int first_sum = 0;
    int second_sum = 0;
    int index;
    va_start(first, count);
    va_copy(second, first);
    for (index = 0; index < count; ++index)
        first_sum += va_arg(first, int);
    for (index = 0; index < count; ++index)
        second_sum += va_arg(second, int);
    va_end(first);
    va_end(second);
    return first_sum == second_sum ? first_sum : 100;
}

static int mixed_values(int marker, ...) {
    va_list list;
    int integer;
    double real;
    long long wide;
    va_start(list, marker);
    integer = va_arg(list, int);
    real = va_arg(list, double);
    wide = va_arg(list, long long);
    va_end(list);
    return integer + (int)real + (int)wide;
}

static int promoted_values(int marker, ...) {
    va_list list;
    int byte;
    int half;
    double real;
    enum Choice choice;
    int narrow;
    unsigned wide;
    va_start(list, marker);
    byte = va_arg(list, int);
    half = va_arg(list, int);
    real = va_arg(list, double);
    choice = va_arg(list, enum Choice);
    narrow = va_arg(list, int);
    wide = va_arg(list, unsigned);
    va_end(list);
    return byte + half + (int)real + choice + narrow +
           (wide == 3000000000U);
}

static int enum_marker(enum Choice marker, ...) {
    va_list list;
    enum Choice choice;
    va_start(list, marker);
    choice = va_arg(list, enum Choice);
    va_end(list);
    return choice;
}

int main(void) {
    char byte = 1;
    unsigned short half = 2;
    float real = 3.0f;
    struct Bits bits;
    bits.narrow = 4;
    bits.wide = 3000000000U;
    return sum_twice(8, 1, 2, 3, 4, 5, 6, 7, 8) +
           mixed_values(0, 7, 8.0, 9LL) +
           promoted_values(0, byte, half, real, CHOICE, bits.narrow, bits.wide) +
           enum_marker(CHOICE, CHOICE);
}
