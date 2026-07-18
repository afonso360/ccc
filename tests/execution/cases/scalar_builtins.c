#if !__has_builtin(__builtin_expect) || !__has_builtin(__builtin_huge_val) || \
    !__has_builtin(__builtin_inff) || !__has_builtin(__builtin_nanf)
#error "required scalar builtins are unavailable"
#endif

static double positive_infinity = __builtin_huge_val();
static float positive_float_infinity = __builtin_inff();
static float quiet_nan = __builtin_nanf("");

int main(void) {
    signed char value = 41;
    long result = __builtin_expect(value++, 1);
    union {
        double floating;
        unsigned long bits;
    } representation;
    union {
        float floating;
        unsigned bits;
    } float_representation;
    representation.floating = positive_infinity;

    if (result != 41 || value != 42) {
        return 1;
    }
    if (representation.bits != 0x7ff0000000000000UL) {
        return 2;
    }
    float_representation.floating = positive_float_infinity;
    if (float_representation.bits != 0x7f800000U) {
        return 3;
    }
    float_representation.floating = quiet_nan;
    if (float_representation.bits != 0x7fc00000U) {
        return 4;
    }
    if (quiet_nan == quiet_nan) {
        return 5;
    }
    return 56;
}
