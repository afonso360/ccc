/* ccc-kernel-benchmark: variadic-call */
/* ccc-kernel-work-unit: variadic-call */
/* ccc-kernel-work-count: 1000000 */
/* ccc-kernel-expected-result: 0xb4ceb671 */

#include <stdarg.h>

_Static_assert(sizeof(unsigned) == 4, "variadic-call requires 32-bit unsigned");

enum {
    WORK_COUNT = 1000000
};

static volatile unsigned seed = 0x6a09e667u;

static unsigned consume(unsigned tag, ...) {
    va_list arguments;
    unsigned unsigned_value;
    int signed_value;
    double promoted_float;
    unsigned *pointer_value;
    unsigned result;

    va_start(arguments, tag);
    unsigned_value = va_arg(arguments, unsigned);
    signed_value = va_arg(arguments, int);
    promoted_float = va_arg(arguments, double);
    pointer_value = va_arg(arguments, unsigned *);
    va_end(arguments);

    result = tag * 0x9e3779b9u + unsigned_value;
    result ^= (unsigned)signed_value * 0x85ebca6bu;
    result += (unsigned)(promoted_float * 2.0) * 0xc2b2ae35u;
    result ^= *pointer_value;
    return (result << 13) | (result >> 19);
}

int main(void) {
    unsigned state = seed;
    unsigned checksum = 0x3c6ef372u;
    unsigned iteration;

    for (iteration = 0; iteration < WORK_COUNT; ++iteration) {
        unsigned unsigned_value = state ^ iteration * 0x7f4a7c15u;
        int signed_value = (int)(iteration & 1023u) - 512;
        float promoted_float = (float)(iteration & 7u) + 0.5f;
        unsigned pointed_value = state + iteration;
        unsigned mixed = consume(
            iteration,
            unsigned_value,
            signed_value,
            promoted_float,
            &pointed_value);

        state = ((state ^ mixed) << 3) | ((state ^ mixed) >> 29);
        state += 0x27d4eb2du;
        checksum += mixed ^ state;
    }

    return (checksum ^ state) != 0xb4ceb671u;
}
