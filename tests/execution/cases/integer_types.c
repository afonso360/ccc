int main(void) {
    _Bool truth = 7;
    char target_char = -1;
    signed char signed_byte = -7;
    unsigned char unsigned_byte = 250;
    short signed_short = -1234;
    unsigned short unsigned_short = 60000;
    int signed_int = -123456;
    unsigned int unsigned_int = 4000000000U;
    long signed_long = -2000000000L;
    unsigned long unsigned_long = 5000000000UL;
    long long signed_long_long = -5000000000LL;
    unsigned long long unsigned_long_long = 10000000000ULL;

    if (truth != 1)
        return 1;
    if (target_char >= 0 || signed_byte + 7 != 0)
        return 2;
    if ((unsigned char)(unsigned_byte + 10) != 4)
        return 3;
    if (signed_short != -1234 || unsigned_short != 60000U)
        return 4;
    if (signed_int != -123456 || unsigned_int != 4000000000U)
        return 5;
    if (signed_long != -2000000000L || unsigned_long != 5000000000UL)
        return 6;
    if (signed_long_long != -5000000000LL
        || unsigned_long_long != 10000000000ULL)
        return 7;
    return 41;
}
