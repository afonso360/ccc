#include <errno.h>
#include <fenv.h>
#include <math.h>
#include <stdint.h>

static volatile double input;
static double (*volatile reference_sqrt)(double) = sqrt;

static double guarded_sqrt(double value) {
    if (value < 0.0)
        return 0.0;
    return sqrt(value);
}

static uint64_t bits(double value) {
    union {
        double floating;
        uint64_t integer;
    } representation;
    representation.floating = value;
    return representation.integer;
}

static int compare(double value, int rounding) {
    double actual;
    double expected;
    int actual_errno;
    int expected_errno;
    int actual_flags;
    int expected_flags;

    if (fesetround(rounding) != 0)
        return 1;
    input = value;
    errno = 0;
    feclearexcept(FE_ALL_EXCEPT);
    actual = guarded_sqrt(input);
    actual_errno = errno;
    actual_flags = fetestexcept(FE_ALL_EXCEPT);

    if (fesetround(rounding) != 0)
        return 2;
    input = value;
    errno = 0;
    feclearexcept(FE_ALL_EXCEPT);
    expected = reference_sqrt(input);
    expected_errno = errno;
    expected_flags = fetestexcept(FE_ALL_EXCEPT);

    return bits(actual) != bits(expected) || actual_errno != expected_errno || actual_flags != expected_flags;
}

int main(void) {
    volatile double negative_zero = -0.0;
    volatile double quiet_nan = NAN;
    volatile double positive_infinity = INFINITY;

    if (compare(negative_zero, FE_TONEAREST) != 0)
        return 10;
    if (compare(quiet_nan, FE_TONEAREST) != 0)
        return 11;
    if (compare(positive_infinity, FE_TONEAREST) != 0)
        return 12;
    if (compare(2.0, FE_DOWNWARD) != 0)
        return 13;
    if (compare(2.0, FE_UPWARD) != 0)
        return 14;
    return 0;
}
