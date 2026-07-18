static double positive_infinity = __builtin_huge_val();

int main(void) {
    signed char value = 41;
    long result = __builtin_expect(value++, 1);
    union {
        double floating;
        unsigned long bits;
    } representation;
    representation.floating = positive_infinity;

    if (result != 41 || value != 42) {
        return 1;
    }
    if (representation.bits != 0x7ff0000000000000UL) {
        return 2;
    }
    return 56;
}
