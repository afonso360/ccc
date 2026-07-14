int main(void) {
    unsigned int bits = 3U;
    int value = 5;
    int old;
    int selected;
    int sequenced;

    old = value++;
    if (old != 5 || value != 6)
        return 1;
    old = value--;
    if (old != 6 || value != 5)
        return 2;
    if (++value != 6 || --value != 5)
        return 3;

    bits *= 4U;
    bits /= 3U;
    bits %= 3U;
    bits += 7U;
    bits -= 3U;
    bits <<= 2;
    bits >>= 1;
    bits &= 14U;
    bits ^= 3U;
    bits |= 4U;
    if (bits != 13U)
        return 4;

    if ((bits & 9U) != 9U || (bits ^ 5U) != 8U || (bits | 2U) != 15U)
        return 5;
    if ((17U % 5U) != 2U || (3U << 3) != 24U || (24U >> 2) != 6U)
        return 6;
    if ((~0U & 15U) != 15U || +value != 5 || -value != -5 || !value)
        return 7;

    selected = bits == 13U ? 20 : 1;
    sequenced = (value = 2, value += 3, value * 2);
    if (selected != 20 || sequenced != 10 || value != 5)
        return 8;
    if (!(value == 5 && bits == 13U) || (value == 0 || bits == 0U))
        return 9;

    return 52;
}
