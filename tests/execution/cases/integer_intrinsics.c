#if !__has_builtin(__builtin_bswap64) || !__has_builtin(__builtin_clz) || \
    !__has_builtin(__builtin_clzl) || !__has_builtin(__builtin_clzll) || \
    !__has_builtin(__builtin_ctz) || !__has_builtin(__builtin_ctzll) || \
    !__has_builtin(__builtin_popcount) || !__has_builtin(__builtin_popcountll) || \
    !__has_builtin(__builtin_prefetch)
#error "required integer builtins are unavailable"
#endif

#if __has_builtin(__builtin_bswap32) || __has_builtin(__builtin_ctzl) || \
    __has_builtin(__builtin_popcountl)
#error "an unselected integer builtin was advertised"
#endif

enum folded_intrinsics {
    folded_swap = __builtin_bswap64(1UL),
    folded_clz = __builtin_clz(1U),
    folded_clzl = __builtin_clzl(1UL),
    folded_clzll = __builtin_clzll(1ULL),
    folded_ctz = __builtin_ctz(0x20U),
    folded_ctzll = __builtin_ctzll(0x100ULL),
    folded_popcount = __builtin_popcount(0xf0U),
    folded_popcountll = __builtin_popcountll(0xf00000000000000fULL)
};

_Static_assert(folded_swap == 0x0100000000000000UL, "folded bswap64");
_Static_assert(folded_clz == 31, "folded clz");
_Static_assert(folded_clzl == 63, "folded clzl");
_Static_assert(folded_clzll == 63, "folded clzll");
_Static_assert(folded_ctzll == 8, "folded ctzll");
_Static_assert(folded_popcount == 4, "folded popcount");
_Static_assert(folded_popcountll == 8, "folded popcountll");

static int folded_array[(folded_clz + folded_clzl + folded_clzll +
                         folded_ctz + folded_ctzll + folded_popcount +
                         folded_popcountll == 182)
                            ? 7
                            : -1];

static int address_evaluations;
static int value;

static void *next_address(void) {
    address_evaluations++;
    return &value;
}

int main(void) {
    if (sizeof(folded_array) != 7 * sizeof(int)) return 9;
    if (__builtin_bswap64(0x0123456789abcdefULL) != 0xefcdab8967452301ULL)
        return 1;
    if (__builtin_clz(0x10U) != 27) return 2;
    if (__builtin_clzl(1UL) != 63) return 3;
    if (__builtin_clzll(1ULL << 60) != 3) return 4;
    if (__builtin_ctz(0x20U) != 5) return 10;
    if (__builtin_ctzll(0x100ULL) != 8) return 5;
    if (__builtin_popcount(0xf0f0U) != 8) return 6;
    if (__builtin_popcountll(0xf00000000000000fULL) != 8) return 7;

    __builtin_prefetch(next_address(), 0, 3);
    if (address_evaluations != 1) return 8;
    __builtin_prefetch((void *)1, 1, 0);
    __builtin_prefetch(0);
    __builtin_prefetch(&value, 4294967296ULL, 4294967299ULL);

    return 61;
}
