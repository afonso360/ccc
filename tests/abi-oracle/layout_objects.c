#define ORACLE_OFFSETOF(type, member) __builtin_offsetof(type, member)

struct OracleRecord {
    char byte;
    int integer;
    long wide;
};

union OracleUnion {
    char byte;
    double floating;
};

struct OracleAfterByte {
    char prefix;
    int first : 20;
    int second : 20;
};

struct OracleMixedTypes {
    unsigned char byte : 3;
    unsigned short half : 9;
    unsigned int word : 17;
    unsigned long wide : 33;
};

struct OracleSigned {
    signed int negative : 5;
    unsigned int positive : 6;
};

struct OracleZeroWidth {
    unsigned int low : 3;
    unsigned int : 0;
    unsigned int high : 5;
};

struct OracleZeroWidthOnly {
    char prefix;
    unsigned int : 0;
    char suffix;
};

struct OracleStraddling {
    unsigned int left : 31;
    unsigned int right : 2;
};

struct OracleAnonymous {
    unsigned int : 3;
    unsigned int named : 5;
    unsigned int : 2;
    unsigned int tail : 4;
};

struct OraclePlainInt {
    int plain : 3;
    unsigned int follower : 5;
    char tail;
};

struct OracleNestedInner {
    unsigned short left : 7;
    signed short right : 6;
};

struct OracleNested {
    char prefix;
    struct OracleNestedInner inner;
    unsigned int tail : 5;
};

union OracleBitfieldUnion {
    unsigned int word : 19;
    signed short signed_half : 11;
    unsigned char byte : 7;
};

#pragma pack(push, 1)
struct OraclePacked {
    char prefix;
    unsigned int low : 20;
    unsigned int high : 20;
    char suffix;
};

struct OraclePackedZeroWidth {
    char prefix;
    unsigned int low : 3;
    unsigned int : 0;
    unsigned int high : 3;
    char suffix;
};
#pragma pack(pop)

const unsigned long abi_layout_values[] = {
    sizeof(_Bool),
    _Alignof(_Bool),
    sizeof(char),
    _Alignof(char),
    sizeof(short),
    _Alignof(short),
    sizeof(int),
    _Alignof(int),
    sizeof(long),
    _Alignof(long),
    sizeof(long long),
    _Alignof(long long),
    sizeof(void *),
    _Alignof(void *),
    sizeof(float),
    _Alignof(float),
    sizeof(double),
    _Alignof(double),
    sizeof(long double),
    _Alignof(long double),
    sizeof(struct OracleRecord),
    _Alignof(struct OracleRecord),
    ORACLE_OFFSETOF(struct OracleRecord, integer),
    ORACLE_OFFSETOF(struct OracleRecord, wide),
    sizeof(union OracleUnion),
    _Alignof(union OracleUnion),
    sizeof(struct OracleAfterByte),
    _Alignof(struct OracleAfterByte),
    sizeof(struct OracleMixedTypes),
    _Alignof(struct OracleMixedTypes),
    sizeof(struct OracleSigned),
    _Alignof(struct OracleSigned),
    sizeof(struct OracleZeroWidth),
    _Alignof(struct OracleZeroWidth),
    sizeof(struct OracleZeroWidthOnly),
    _Alignof(struct OracleZeroWidthOnly),
    ORACLE_OFFSETOF(struct OracleZeroWidthOnly, suffix),
    sizeof(struct OracleStraddling),
    _Alignof(struct OracleStraddling),
    sizeof(struct OracleAnonymous),
    _Alignof(struct OracleAnonymous),
    sizeof(struct OraclePlainInt),
    _Alignof(struct OraclePlainInt),
    ORACLE_OFFSETOF(struct OraclePlainInt, tail),
    sizeof(struct OracleNestedInner),
    _Alignof(struct OracleNestedInner),
    sizeof(struct OracleNested),
    _Alignof(struct OracleNested),
    ORACLE_OFFSETOF(struct OracleNested, inner),
    sizeof(union OracleBitfieldUnion),
    _Alignof(union OracleBitfieldUnion),
    sizeof(struct OraclePacked),
    _Alignof(struct OraclePacked),
    ORACLE_OFFSETOF(struct OraclePacked, suffix),
    sizeof(struct OraclePackedZeroWidth),
    _Alignof(struct OraclePackedZeroWidth),
    ORACLE_OFFSETOF(struct OraclePackedZeroWidth, suffix),
};

#define ORACLE_SAMPLE(type, group, name, ...) \
    const type abi_##group##_##name = {__VA_ARGS__}

ORACLE_SAMPLE(struct OracleAfterByte, after_byte, zero, 0);
ORACLE_SAMPLE(struct OracleAfterByte, after_byte, first_one, .first = 1);
ORACLE_SAMPLE(struct OracleAfterByte, after_byte, first_max, .first = 524287);
ORACLE_SAMPLE(struct OracleAfterByte, after_byte, first_negative, .first = -1);
ORACLE_SAMPLE(struct OracleAfterByte, after_byte, second_one, .second = 1);
ORACLE_SAMPLE(struct OracleAfterByte, after_byte, second_max, .second = 524287);
ORACLE_SAMPLE(struct OracleAfterByte, after_byte, second_negative, .second = -1);

ORACLE_SAMPLE(struct OracleMixedTypes, mixed, zero, 0);
ORACLE_SAMPLE(struct OracleMixedTypes, mixed, byte_one, .byte = 1U);
ORACLE_SAMPLE(struct OracleMixedTypes, mixed, byte_max, .byte = 7U);
ORACLE_SAMPLE(struct OracleMixedTypes, mixed, half_one, .half = 1U);
ORACLE_SAMPLE(struct OracleMixedTypes, mixed, half_max, .half = 511U);
ORACLE_SAMPLE(struct OracleMixedTypes, mixed, word_one, .word = 1U);
ORACLE_SAMPLE(struct OracleMixedTypes, mixed, word_max, .word = 131071U);
ORACLE_SAMPLE(struct OracleMixedTypes, mixed, wide_one, .wide = 1UL);
ORACLE_SAMPLE(struct OracleMixedTypes, mixed, wide_max, .wide = 0x1ffffffffUL);

ORACLE_SAMPLE(struct OracleSigned, signed, zero, 0);
ORACLE_SAMPLE(struct OracleSigned, signed, negative_one, .negative = 1);
ORACLE_SAMPLE(struct OracleSigned, signed, negative_max, .negative = 15);
ORACLE_SAMPLE(struct OracleSigned, signed, negative_minus_one, .negative = -1);
ORACLE_SAMPLE(struct OracleSigned, signed, negative_min, .negative = -16);
ORACLE_SAMPLE(struct OracleSigned, signed, positive_one, .positive = 1U);
ORACLE_SAMPLE(struct OracleSigned, signed, positive_max, .positive = 63U);

ORACLE_SAMPLE(struct OracleZeroWidth, zero_width, zero, 0);
ORACLE_SAMPLE(struct OracleZeroWidth, zero_width, low_one, .low = 1U);
ORACLE_SAMPLE(struct OracleZeroWidth, zero_width, low_max, .low = 7U);
ORACLE_SAMPLE(struct OracleZeroWidth, zero_width, high_one, .high = 1U);
ORACLE_SAMPLE(struct OracleZeroWidth, zero_width, high_max, .high = 31U);

ORACLE_SAMPLE(struct OracleStraddling, straddling, zero, 0);
ORACLE_SAMPLE(struct OracleStraddling, straddling, left_one, .left = 1U);
ORACLE_SAMPLE(struct OracleStraddling, straddling, left_max, .left = 0x7fffffffU);
ORACLE_SAMPLE(struct OracleStraddling, straddling, right_one, .right = 1U);
ORACLE_SAMPLE(struct OracleStraddling, straddling, right_max, .right = 3U);

ORACLE_SAMPLE(struct OracleAnonymous, anonymous, zero, 0);
ORACLE_SAMPLE(struct OracleAnonymous, anonymous, named_one, .named = 1U);
ORACLE_SAMPLE(struct OracleAnonymous, anonymous, named_max, .named = 31U);
ORACLE_SAMPLE(struct OracleAnonymous, anonymous, tail_one, .tail = 1U);
ORACLE_SAMPLE(struct OracleAnonymous, anonymous, tail_max, .tail = 15U);

ORACLE_SAMPLE(struct OraclePlainInt, plain, zero, 0);
ORACLE_SAMPLE(struct OraclePlainInt, plain, signed_one, .plain = 1);
ORACLE_SAMPLE(struct OraclePlainInt, plain, signed_negative, .plain = -1);
ORACLE_SAMPLE(struct OraclePlainInt, plain, follower_max, .follower = 31U);
ORACLE_SAMPLE(struct OraclePlainInt, plain, tail_one, .tail = 1);

ORACLE_SAMPLE(struct OracleNested, nested, zero, 0);
ORACLE_SAMPLE(struct OracleNested, nested, prefix_one, .prefix = 1);
ORACLE_SAMPLE(struct OracleNested, nested, inner_left_one, .inner = {.left = 1U});
ORACLE_SAMPLE(struct OracleNested, nested, inner_left_max, .inner = {.left = 127U});
ORACLE_SAMPLE(struct OracleNested, nested, inner_right_negative,
              .inner = {.right = -1});
ORACLE_SAMPLE(struct OracleNested, nested, tail_max, .tail = 31U);

ORACLE_SAMPLE(union OracleBitfieldUnion, bitfield_union, zero, 0);
ORACLE_SAMPLE(union OracleBitfieldUnion, bitfield_union, word_one, .word = 1U);
ORACLE_SAMPLE(union OracleBitfieldUnion, bitfield_union, word_max, .word = 0x7ffffU);
ORACLE_SAMPLE(union OracleBitfieldUnion, bitfield_union, signed_negative,
              .signed_half = -1);
ORACLE_SAMPLE(union OracleBitfieldUnion, bitfield_union, byte_max, .byte = 127U);

ORACLE_SAMPLE(struct OraclePacked, packed, zero, 0);
ORACLE_SAMPLE(struct OraclePacked, packed, prefix_one, .prefix = 1);
ORACLE_SAMPLE(struct OraclePacked, packed, low_one, .low = 1U);
ORACLE_SAMPLE(struct OraclePacked, packed, low_max, .low = 0xfffffU);
ORACLE_SAMPLE(struct OraclePacked, packed, high_one, .high = 1U);
ORACLE_SAMPLE(struct OraclePacked, packed, high_max, .high = 0xfffffU);
ORACLE_SAMPLE(struct OraclePacked, packed, suffix_one, .suffix = 1);

ORACLE_SAMPLE(struct OraclePackedZeroWidth, packed_zero, zero, 0);
ORACLE_SAMPLE(struct OraclePackedZeroWidth, packed_zero, low_max, .low = 7U);
ORACLE_SAMPLE(struct OraclePackedZeroWidth, packed_zero, high_max, .high = 7U);
ORACLE_SAMPLE(struct OraclePackedZeroWidth, packed_zero, suffix_one, .suffix = 1);
