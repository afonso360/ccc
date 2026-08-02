#include <errno.h>
#include <fenv.h>
#include <math.h>
#include <stdint.h>

static volatile double input_double;
static volatile double sign_double;
static volatile float input_float;
static volatile float sign_float;

static double (*volatile reference_fabs)(double) = fabs;
static double (*volatile reference_copysign)(double, double) = copysign;
static double (*volatile reference_ceil)(double) = ceil;
static double (*volatile reference_floor)(double) = floor;
static double (*volatile reference_trunc)(double) = trunc;
static float (*volatile reference_fabsf)(float) = fabsf;
static float (*volatile reference_copysignf)(float, float) = copysignf;
static float (*volatile reference_ceilf)(float) = ceilf;
static float (*volatile reference_floorf)(float) = floorf;
static float (*volatile reference_truncf)(float) = truncf;

static double native_fabs(double value) { return fabs(value); }
static double native_copysign(double value, double sign) { return copysign(value, sign); }
static double native_ceil(double value) { return ceil(value); }
static double native_floor(double value) { return floor(value); }
static double native_trunc(double value) { return trunc(value); }
static float native_fabsf(float value) { return fabsf(value); }
static float native_copysignf(float value, float sign) { return copysignf(value, sign); }
static float native_ceilf(float value) { return ceilf(value); }
static float native_floorf(float value) { return floorf(value); }
static float native_truncf(float value) { return truncf(value); }

static uint64_t double_bits(double value) {
    union {
        double floating;
        uint64_t integer;
    } representation;
    representation.floating = value;
    return representation.integer;
}

static uint32_t float_bits(float value) {
    union {
        float floating;
        uint32_t integer;
    } representation;
    representation.floating = value;
    return representation.integer;
}

#define CHECK_UNARY(TYPE, BITS, INPUT, NATIVE, REFERENCE, VALUE, ROUNDING, ALLOW_INEXACT) \
    do {                                                                               \
        TYPE actual;                                                                   \
        TYPE expected;                                                                 \
        int actual_errno;                                                              \
        int expected_errno;                                                            \
        int actual_flags;                                                              \
        int expected_flags;                                                            \
        if (fesetround(ROUNDING) != 0)                                                 \
            return 1;                                                                  \
        INPUT = (VALUE);                                                               \
        errno = 0;                                                                     \
        feclearexcept(FE_ALL_EXCEPT);                                                  \
        actual = NATIVE(INPUT);                                                        \
        actual_errno = errno;                                                          \
        actual_flags = fetestexcept(FE_ALL_EXCEPT);                                    \
        if (fesetround(ROUNDING) != 0)                                                 \
            return 2;                                                                  \
        INPUT = (VALUE);                                                               \
        errno = 0;                                                                     \
        feclearexcept(FE_ALL_EXCEPT);                                                  \
        expected = REFERENCE(INPUT);                                                   \
        expected_errno = errno;                                                        \
        expected_flags = fetestexcept(FE_ALL_EXCEPT);                                  \
        /* C permits ceil/floor/trunc to differ only in FE_INEXACT. */                 \
        if (BITS(actual) != BITS(expected) || actual_errno != expected_errno ||       \
            (actual_flags & ~FE_INEXACT) != (expected_flags & ~FE_INEXACT) ||         \
            (!(ALLOW_INEXACT) && actual_flags != expected_flags))                      \
            return 3;                                                                  \
    } while (0)

#define CHECK_COPYSIGN(TYPE, BITS, INPUT, SIGN_INPUT, NATIVE, REFERENCE, VALUE, SIGN, ROUNDING) \
    do {                                                                                         \
        TYPE actual;                                                                             \
        TYPE expected;                                                                           \
        int actual_errno;                                                                        \
        int expected_errno;                                                                      \
        int actual_flags;                                                                        \
        int expected_flags;                                                                      \
        if (fesetround(ROUNDING) != 0)                                                           \
            return 4;                                                                            \
        INPUT = (VALUE);                                                                         \
        SIGN_INPUT = (SIGN);                                                                     \
        errno = 0;                                                                               \
        feclearexcept(FE_ALL_EXCEPT);                                                            \
        actual = NATIVE(INPUT, SIGN_INPUT);                                                      \
        actual_errno = errno;                                                                    \
        actual_flags = fetestexcept(FE_ALL_EXCEPT);                                              \
        if (fesetround(ROUNDING) != 0)                                                           \
            return 5;                                                                            \
        INPUT = (VALUE);                                                                         \
        SIGN_INPUT = (SIGN);                                                                     \
        errno = 0;                                                                               \
        feclearexcept(FE_ALL_EXCEPT);                                                            \
        expected = REFERENCE(INPUT, SIGN_INPUT);                                                 \
        expected_errno = errno;                                                                  \
        expected_flags = fetestexcept(FE_ALL_EXCEPT);                                            \
        if (BITS(actual) != BITS(expected) || actual_errno != expected_errno ||                 \
            actual_flags != expected_flags)                                                      \
            return 6;                                                                            \
    } while (0)

static int check_values_double(double value, int allow_rounding_inexact) {
    CHECK_UNARY(double, double_bits, input_double, native_fabs, reference_fabs, value, FE_TONEAREST, 0);
    CHECK_COPYSIGN(double, double_bits, input_double, sign_double, native_copysign, reference_copysign, value, -0.0, FE_TONEAREST);
    CHECK_UNARY(double, double_bits, input_double, native_ceil, reference_ceil, value, FE_TONEAREST, allow_rounding_inexact);
    CHECK_UNARY(double, double_bits, input_double, native_floor, reference_floor, value, FE_TONEAREST, allow_rounding_inexact);
    CHECK_UNARY(double, double_bits, input_double, native_trunc, reference_trunc, value, FE_TONEAREST, allow_rounding_inexact);
    return 0;
}

static int check_values_float(float value, int allow_rounding_inexact) {
    CHECK_UNARY(float, float_bits, input_float, native_fabsf, reference_fabsf, value, FE_TONEAREST, 0);
    CHECK_COPYSIGN(float, float_bits, input_float, sign_float, native_copysignf, reference_copysignf, value, -0.0f, FE_TONEAREST);
    CHECK_UNARY(float, float_bits, input_float, native_ceilf, reference_ceilf, value, FE_TONEAREST, allow_rounding_inexact);
    CHECK_UNARY(float, float_bits, input_float, native_floorf, reference_floorf, value, FE_TONEAREST, allow_rounding_inexact);
    CHECK_UNARY(float, float_bits, input_float, native_truncf, reference_truncf, value, FE_TONEAREST, allow_rounding_inexact);
    return 0;
}

int main(void) {
    volatile double double_negative_zero = -0.0;
    volatile double double_positive_zero = 0.0;
    volatile double double_positive_infinity = INFINITY;
    volatile double double_negative_infinity = -INFINITY;
    volatile double double_quiet_nan = NAN;
    volatile double double_subnormal = 0x1p-1074;
    volatile float float_negative_zero = -0.0f;
    volatile float float_positive_zero = 0.0f;
    volatile float float_positive_infinity = INFINITY;
    volatile float float_negative_infinity = -INFINITY;
    volatile float float_quiet_nan = NAN;
    volatile float float_subnormal = 0x1p-149f;
    int result;

    if ((result = check_values_double(double_negative_zero, 0)) != 0) return 10 + result;
    if ((result = check_values_double(double_positive_zero, 0)) != 0) return 20 + result;
    if ((result = check_values_double(-1.75, 1)) != 0) return 30 + result;
    if ((result = check_values_double(2.0, 0)) != 0) return 40 + result;
    if ((result = check_values_double(double_positive_infinity, 0)) != 0) return 50 + result;
    if ((result = check_values_double(double_negative_infinity, 0)) != 0) return 60 + result;
    if ((result = check_values_double(double_quiet_nan, 0)) != 0) return 70 + result;
    if ((result = check_values_double(double_subnormal, 1)) != 0) return 80 + result;

    if ((result = check_values_float(float_negative_zero, 0)) != 0) return 90 + result;
    if ((result = check_values_float(float_positive_zero, 0)) != 0) return 100 + result;
    if ((result = check_values_float(-1.75f, 1)) != 0) return 110 + result;
    if ((result = check_values_float(2.0f, 0)) != 0) return 120 + result;
    if ((result = check_values_float(float_positive_infinity, 0)) != 0) return 130 + result;
    if ((result = check_values_float(float_negative_infinity, 0)) != 0) return 140 + result;
    if ((result = check_values_float(float_quiet_nan, 0)) != 0) return 150 + result;
    if ((result = check_values_float(float_subnormal, 1)) != 0) return 160 + result;

    CHECK_UNARY(double, double_bits, input_double, native_ceil, reference_ceil, -1.75, FE_DOWNWARD, 1);
    CHECK_UNARY(double, double_bits, input_double, native_floor, reference_floor, -1.75, FE_UPWARD, 1);
    CHECK_UNARY(double, double_bits, input_double, native_trunc, reference_trunc, -1.75, FE_TOWARDZERO, 1);
    CHECK_UNARY(float, float_bits, input_float, native_ceilf, reference_ceilf, -1.75f, FE_DOWNWARD, 1);
    CHECK_UNARY(float, float_bits, input_float, native_floorf, reference_floorf, -1.75f, FE_UPWARD, 1);
    CHECK_UNARY(float, float_bits, input_float, native_truncf, reference_truncf, -1.75f, FE_TOWARDZERO, 1);
    return 0;
}
