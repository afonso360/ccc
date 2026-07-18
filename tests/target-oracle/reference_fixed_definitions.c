#include "abi_types.h"

long ref_scalar_mix(
    signed char small, unsigned short wide, const long *pointer,
    enum OracleNumber number) {
    return small + wide + *pointer + number;
}

long ref_pair_after_seven(
    long a0, long a1, long a2, long a3, long a4, long a5, long a6,
    struct Pair value) {
    return a0 + a1 + a2 + a3 + a4 + a5 + a6 + value.first + value.second;
}

long ref_ints_after_eight(
    long a0, long a1, long a2, long a3, long a4, long a5, long a6, long a7,
    int a8, int a9) {
    return a0 + a1 + a2 + a3 + a4 + a5 + a6 + a7 + a8 + a9;
}

long ref_hfa_after_seven(
    double a0, double a1, double a2, double a3, double a4, double a5, double a6,
    struct Hfa value) {
    return (long)(a0 + a1 + a2 + a3 + a4 + a5 + a6 + value.first + value.second);
}

long ref_hfa_union_after_seven(
    double a0, double a1, double a2, double a3, double a4, double a5, double a6,
    union HfaUnion value) {
    return (long)(a0 + a1 + a2 + a3 + a4 + a5 + a6 +
                  value.pair[0] + value.pair[1]);
}

struct Pair ref_pair_transform(struct Pair value) {
    value.first += 3;
    value.second += 5;
    return value;
}

struct Big ref_big_transform(struct Big value) {
    value.first += 3;
    value.second += 5;
    value.third += 7;
    return value;
}

long ref_mixed_after_eight(
    double a0, double a1, double a2, double a3,
    double a4, double a5, double a6, double a7,
    struct Mixed value) {
    return (long)(a0 + a1 + a2 + a3 + a4 + a5 + a6 + a7 +
                  value.floating + value.integer);
}
