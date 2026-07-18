#include "abi_types.h"

int main(void) {
    struct Pair pair = {8, 9};
    struct Hfa hfa = {8.0, 9.0};
    union HfaUnion hfa_union;
    struct Big big = {1, 2, 3};
    struct Mixed mixed = {9.0, 10};
    struct Pair transformed;
    struct Big big_transformed;
    long pointed = 11;
    hfa_union.pair[0] = 8.0;
    hfa_union.pair[1] = 9.0;
    if (ccc_scalar_mix(-3, 65000, &pointed, ORACLE_SEVEN) != 65015) return 10;
    if (ccc_pair_after_seven(1, 2, 3, 4, 5, 6, 7, pair) != 45) return 11;
    if (ccc_ints_after_eight(1, 2, 3, 4, 5, 6, 7, 8, 9, 10) != 55) return 17;
    if (ccc_hfa_after_seven(1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, hfa) != 45) return 12;
    if (ccc_hfa_union_after_seven(1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, hfa_union) != 45) return 16;
    transformed = ccc_pair_transform(pair);
    if (transformed.first != 11 || transformed.second != 14) return 13;
    big_transformed = ccc_big_transform(big);
    if (big_transformed.first != 4 || big_transformed.second != 7 ||
        big_transformed.third != 10) return 14;
    if (ccc_mixed_after_eight(1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, mixed) != 55) return 15;
    return 0;
}
