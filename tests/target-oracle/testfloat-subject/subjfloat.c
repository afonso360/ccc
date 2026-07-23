/*
 * TestFloat subject adapter for CCC's x86 extended-precision operations.
 *
 * This file is compiled by CCC and linked only into the independent test
 * harness.  Berkeley SoftFloat remains on the verifier side of the boundary.
 */

#include <stdbool.h>
#include <stdint.h>

#include "softfloat.h"
#include "subjfloat.h"

enum {
    CCC_FE_INVALID = 0x01,
    CCC_FE_DIVBYZERO = 0x04,
    CCC_FE_OVERFLOW = 0x08,
    CCC_FE_UNDERFLOW = 0x10,
    CCC_FE_INEXACT = 0x20,
    CCC_FE_TONEAREST = 0x000,
    CCC_FE_DOWNWARD = 0x400,
    CCC_FE_UPWARD = 0x800,
    CCC_FE_TOWARDZERO = 0xc00,
};

_Noreturn void abort(void);
int feclearexcept(int);
int feraiseexcept(int);
int fesetround(int);
int fetestexcept(int);

_Static_assert(sizeof(long double) == sizeof(extFloat80_t),
    "TestFloat extF80 and native long double sizes must match");
_Static_assert(sizeof(float) == sizeof(float32_t),
    "TestFloat f32 and native float sizes must match");
_Static_assert(sizeof(double) == sizeof(float64_t),
    "TestFloat f64 and native double sizes must match");

static long double load_extF80(const extFloat80_t *input)
{
    long double value;
    __builtin_memcpy(&value, input, sizeof(value));
    return value;
}

static void store_extF80(extFloat80_t *output, long double value)
{
    __builtin_memcpy(output, &value, sizeof(value));
}

static float load_f32(float32_t input)
{
    float value;
    __builtin_memcpy(&value, &input, sizeof(value));
    return value;
}

static float32_t store_f32(float value)
{
    float32_t output;
    __builtin_memcpy(&output, &value, sizeof(output));
    return output;
}

static double load_f64(float64_t input)
{
    double value;
    __builtin_memcpy(&value, &input, sizeof(value));
    return value;
}

static float64_t store_f64(double value)
{
    float64_t output;
    __builtin_memcpy(&output, &value, sizeof(output));
    return output;
}

static void require_success(int status)
{
    if (status != 0) {
        abort();
    }
}

void subjfloat_setRoundingMode(uint_fast8_t rounding_mode)
{
    int native_mode;

    switch (rounding_mode) {
    case softfloat_round_near_even:
        native_mode = CCC_FE_TONEAREST;
        break;
    case softfloat_round_minMag:
        native_mode = CCC_FE_TOWARDZERO;
        break;
    case softfloat_round_min:
        native_mode = CCC_FE_DOWNWARD;
        break;
    case softfloat_round_max:
        native_mode = CCC_FE_UPWARD;
        break;
    default:
        abort();
    }
    require_success(fesetround(native_mode));
}

void subjfloat_setExtF80RoundingPrecision(uint_fast8_t rounding_precision)
{
    /* The x86-64 ABI uses the full 64-bit extended significand. */
    if (rounding_precision != 80) {
        abort();
    }
}

uint_fast8_t subjfloat_clearExceptionFlags(void)
{
    const int all_exceptions = CCC_FE_INVALID | CCC_FE_DIVBYZERO
        | CCC_FE_OVERFLOW | CCC_FE_UNDERFLOW | CCC_FE_INEXACT;
    int native_flags = fetestexcept(all_exceptions);
    uint_fast8_t flags = 0;

    require_success(feclearexcept(all_exceptions));
    if (native_flags & CCC_FE_INVALID) {
        flags |= softfloat_flag_invalid;
    }
    if (native_flags & CCC_FE_DIVBYZERO) {
        flags |= softfloat_flag_infinite;
    }
    if (native_flags & CCC_FE_OVERFLOW) {
        flags |= softfloat_flag_overflow;
    }
    if (native_flags & CCC_FE_UNDERFLOW) {
        flags |= softfloat_flag_underflow;
    }
    if (native_flags & CCC_FE_INEXACT) {
        flags |= softfloat_flag_inexact;
    }
    return flags;
}

void subj_ui64_to_extF80M(uint64_t input, extFloat80_t *output)
{
    store_extF80(output, input);
}

void subj_i64_to_extF80M(int64_t input, extFloat80_t *output)
{
    store_extF80(output, input);
}

void subj_f32_to_extF80M(float32_t input, extFloat80_t *output)
{
    store_extF80(output, load_f32(input));
}

void subj_f64_to_extF80M(float64_t input, extFloat80_t *output)
{
    store_extF80(output, load_f64(input));
}

uint_fast64_t subj_extF80M_to_ui64_rx_minMag(const extFloat80_t *input)
{
    long double value = load_extF80(input);

    if (value != value || !(value > -1.0L && value < 0x1p64L)) {
        require_success(feraiseexcept(CCC_FE_INVALID));
        return UINT64_MAX;
    }
    return value;
}

int_fast64_t subj_extF80M_to_i64_rx_minMag(const extFloat80_t *input)
{
    long double value = load_extF80(input);

    if (value != value || !(value >= -0x1p63L && value < 0x1p63L)) {
        require_success(feraiseexcept(CCC_FE_INVALID));
        return INT64_MIN;
    }
    return value;
}

float32_t subj_extF80M_to_f32(const extFloat80_t *input)
{
    return store_f32(load_extF80(input));
}

float64_t subj_extF80M_to_f64(const extFloat80_t *input)
{
    return store_f64(load_extF80(input));
}

void subj_extF80M_add(
    const extFloat80_t *left,
    const extFloat80_t *right,
    extFloat80_t *output)
{
    store_extF80(output, load_extF80(left) + load_extF80(right));
}

void subj_extF80M_sub(
    const extFloat80_t *left,
    const extFloat80_t *right,
    extFloat80_t *output)
{
    store_extF80(output, load_extF80(left) - load_extF80(right));
}

void subj_extF80M_mul(
    const extFloat80_t *left,
    const extFloat80_t *right,
    extFloat80_t *output)
{
    store_extF80(output, load_extF80(left) * load_extF80(right));
}

void subj_extF80M_div(
    const extFloat80_t *left,
    const extFloat80_t *right,
    extFloat80_t *output)
{
    store_extF80(output, load_extF80(left) / load_extF80(right));
}

bool subj_extF80M_eq(const extFloat80_t *left, const extFloat80_t *right)
{
    return load_extF80(left) == load_extF80(right);
}

bool subj_extF80M_le(const extFloat80_t *left, const extFloat80_t *right)
{
    return load_extF80(left) <= load_extF80(right);
}

bool subj_extF80M_lt(const extFloat80_t *left, const extFloat80_t *right)
{
    return load_extF80(left) < load_extF80(right);
}
