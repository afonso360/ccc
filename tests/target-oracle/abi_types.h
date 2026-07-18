#ifndef CCC_TARGET_ORACLE_ABI_TYPES_H
#define CCC_TARGET_ORACLE_ABI_TYPES_H

struct Pair {
    long first;
    long second;
};

struct Hfa {
    double first;
    double second;
};

union HfaUnion {
    double scalar;
    double pair[2];
};

struct Big {
    long first;
    long second;
    long third;
};

struct Mixed {
    double floating;
    int integer;
};

enum OracleNumber {
    ORACLE_SEVEN = 7
};

long ccc_scalar_mix(signed char, unsigned short, const long *, enum OracleNumber);
long ccc_pair_after_seven(long, long, long, long, long, long, long, struct Pair);
long ccc_ints_after_eight(long, long, long, long, long, long, long, long, int, int);
long ccc_hfa_after_seven(double, double, double, double, double, double, double, struct Hfa);
long ccc_hfa_union_after_seven(double, double, double, double, double, double, double, union HfaUnion);
struct Pair ccc_pair_transform(struct Pair);
struct Big ccc_big_transform(struct Big);
long ccc_mixed_after_eight(double, double, double, double, double, double, double, double, struct Mixed);

long ref_scalar_mix(signed char, unsigned short, const long *, enum OracleNumber);
long ref_pair_after_seven(long, long, long, long, long, long, long, struct Pair);
long ref_ints_after_eight(long, long, long, long, long, long, long, long, int, int);
long ref_hfa_after_seven(double, double, double, double, double, double, double, struct Hfa);
long ref_hfa_union_after_seven(double, double, double, double, double, double, double, union HfaUnion);
struct Pair ref_pair_transform(struct Pair);
struct Big ref_big_transform(struct Big);
long ref_mixed_after_eight(double, double, double, double, double, double, double, double, struct Mixed);

long ccc_collect(int, ...);
long ref_collect(int, ...);
int ccc_format(char *, unsigned long, const char *, ...);
int ccc_unwind_entry(int);
int ccc_unwind_variadic(int, ...);
int ref_unwind_variadic(int, ...);
int target_oracle_unwind_probe(int);

#endif
