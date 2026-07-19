#ifndef CCC_TARGET_ORACLE_X86_F80_ABI_H
#define CCC_TARGET_ORACLE_X86_F80_ABI_H

struct F80Box {
    long double value;
    unsigned long tag;
};

typedef long double (*F80Unary)(long double);
typedef long double (*F80Fixed)(long, double, long double, int, long double);

enum {
    ORACLE_FE_DOWNWARD = 0x400,
    ORACLE_FE_UPWARD = 0x800
};

int fegetround(void);
int fesetround(int);

long double ccc_f80_fixed(long, double, long double, int, long double);
struct F80Box ccc_f80_box(long, struct F80Box, long double, int);
long double ccc_f80_apply(F80Unary, long double);
long double ccc_f80_variadic(long double, int, ...);
long double ccc_f80_arithmetic(long double);
int ccc_f80_relations(long double, long double);
unsigned ccc_f80_comparison_mask(long double, long double);
int ccc_f80_equal(long double, long double);
int ccc_f80_less(long double, long double);
long ccc_f80_to_signed(long double);
unsigned long ccc_f80_unsigned_roundtrip(unsigned long);
__int128 ccc_f80_signed128_roundtrip(__int128);
unsigned __int128 ccc_f80_unsigned128_roundtrip(unsigned __int128);
double ccc_f80_to_double(long double);
long double ccc_f80_from_float(float);
long double ccc_f80_volatile_roundtrip(volatile long double *, long double);
int ccc_f80_rounding_probe(void);

long double ref_f80_fixed(long, double, long double, int, long double);
struct F80Box ref_f80_box(long, struct F80Box, long double, int);
long double ref_f80_apply(F80Unary, long double);
long double ref_f80_variadic(long double, int, ...);
long double ref_f80_arithmetic(long double);
int ref_f80_relations(long double, long double);
unsigned ref_f80_comparison_mask(long double, long double);
long ref_f80_to_signed(long double);
unsigned long ref_f80_unsigned_roundtrip(unsigned long);
__int128 ref_f80_signed128_roundtrip(__int128);
unsigned __int128 ref_f80_unsigned128_roundtrip(unsigned __int128);
double ref_f80_to_double(long double);
long double ref_f80_from_float(float);
long double ref_f80_volatile_roundtrip(volatile long double *, long double);

#endif
