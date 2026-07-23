#include "x86_f80_abi.h"

static volatile long double volatile_storage;

static long double ccc_bump(long double value) {
    return value + 0x1p-63L;
}

static int ccc_rounding_probe(void) {
    volatile long double one = 1.0L;
    volatile long double half_ulp = 0x1p-64L;
    int original = fegetround();
    if (original < 0) return 1;
    if (fesetround(ORACLE_FE_UPWARD) != 0) return 2;
    long double upward = one + half_ulp;
    if (fegetround() != ORACLE_FE_UPWARD) {
        fesetround(original);
        return 3;
    }
    if (fesetround(ORACLE_FE_DOWNWARD) != 0) {
        fesetround(original);
        return 4;
    }
    long double downward = one + half_ulp;
    if (fegetround() != ORACLE_FE_DOWNWARD) {
        fesetround(original);
        return 5;
    }
    if (fesetround(original) != 0) return 6;
    if (!(upward > downward)) return 7;
    return 0;
}

int main(void) {
    long double precise = 0x1.0000000000000002p0L;
    F80Fixed indirect = ref_f80_fixed;
    long double fixed = indirect(7, 0.5, precise, -3, 2.0L);
    if (fixed != precise || !(fixed > 1.0L)) return 101;

    struct F80Box input;
    input.value = 1.25L;
    input.tag = 18;
    struct F80Box output = ref_f80_box(3, input, 2.75L, 4);
    if (output.value != 11.0L || output.tag != 25) return 102;

    if (ref_f80_apply(ccc_bump, 1.0L) != precise) return 103;
    if (ref_f80_variadic(0.5L, 3, 1.25L, -2.5L, 4.0L) != 3.25L) return 104;
    if (ref_f80_arithmetic(3.0L) != -8.0L) return 105;
    if (ref_f80_relations(2.0L, 3.0L) != 0) return 106;
    volatile long double zero = 0.0L;
    long double unordered = zero / zero;
    if (ref_f80_comparison_mask(unordered, 1.0L) != 96) return 107;
    if (ref_f80_to_signed(-12.75L) != -12) return 108;
    if (ref_f80_unsigned_roundtrip(~0UL) != ~0UL) return 109;
    __int128 signed_wide = -((__int128)1 << 100);
    if (ref_f80_signed128_roundtrip(signed_wide) != signed_wide) return 110;
    unsigned __int128 unsigned_wide = (unsigned __int128)1 << 127;
    if (ref_f80_unsigned128_roundtrip(unsigned_wide) != unsigned_wide) return 111;
    if (ref_f80_to_double(1.25L) != 1.25) return 112;
    if (ref_f80_from_float(1.5f) != 1.5L) return 113;
    if (ref_f80_volatile_roundtrip(&volatile_storage, precise) != precise) return 114;
    if (ccc_rounding_probe() != 0) return 115;
    return 0;
}
