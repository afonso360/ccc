#include <fenv.h>

#include "x86_f80_abi.h"

_Static_assert(FE_DOWNWARD == ORACLE_FE_DOWNWARD, "x86 FE_DOWNWARD value");
_Static_assert(FE_UPWARD == ORACLE_FE_UPWARD, "x86 FE_UPWARD value");

static volatile long double volatile_storage;

static long double reference_bump(long double value) {
    return value + 0x1p-63L;
}

int main(void) {
    long double precise = 0x1.0000000000000002p0L;
    F80Fixed indirect = ccc_f80_fixed;
    long double fixed = indirect(7, 0.5, precise, -3, 2.0L);
    if (fixed != precise || !(fixed > 1.0L)) return 81;

    struct F80Box input;
    input.value = 1.25L;
    input.tag = 18;
    struct F80Box output = ccc_f80_box(3, input, 2.75L, 4);
    if (output.value != 11.0L || output.tag != 25) return 82;

    if (ccc_f80_apply(reference_bump, 1.0L) != precise) return 83;
    if (ccc_f80_variadic(0.5L, 3, 1.25L, -2.5L, 4.0L) != 3.25L) return 84;
    if (ccc_f80_arithmetic(3.0L) != -8.0L) return 85;
    if (ccc_f80_relations(2.0L, 3.0L) != 0) return 86;
    volatile long double zero = 0.0L;
    long double unordered = zero / zero;
    if (ccc_f80_comparison_mask(unordered, 1.0L) != 96) return 87;
    if (feclearexcept(FE_ALL_EXCEPT) != 0) return 97;
    if (ccc_f80_equal(unordered, 1.0L)) return 98;
    if (fetestexcept(FE_INVALID) != 0) return 99;
    if (feclearexcept(FE_ALL_EXCEPT) != 0) return 100;
    if (ccc_f80_less(unordered, 1.0L)) return 116;
    if (fetestexcept(FE_INVALID) == 0) return 117;
    if (ccc_f80_to_signed(-12.75L) != -12) return 88;
    if (ccc_f80_unsigned_roundtrip(~0UL) != ~0UL) return 89;
    __int128 signed_wide = -((__int128)1 << 100);
    if (ccc_f80_signed128_roundtrip(signed_wide) != signed_wide) return 90;
    unsigned __int128 unsigned_wide = (unsigned __int128)1 << 127;
    if (ccc_f80_unsigned128_roundtrip(unsigned_wide) != unsigned_wide) return 91;
    if (ccc_f80_to_double(1.25L) != 1.25) return 92;
    if (ccc_f80_from_float(1.5f) != 1.5L) return 93;
    if (ccc_f80_volatile_roundtrip(&volatile_storage, precise) != precise) return 94;
    int original_rounding = fegetround();
    if (ccc_f80_rounding_probe() != 0) return 95;
    if (fegetround() != original_rounding) return 96;
    return 0;
}
