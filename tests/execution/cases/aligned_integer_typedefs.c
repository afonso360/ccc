typedef __attribute__((aligned(1))) unsigned short unalign16;
typedef __attribute__((aligned(1))) unsigned int unalign32;
typedef __attribute__((aligned(1))) unsigned long unalign64;

struct wire_values {
    unsigned char tag;
    unalign16 half;
    unalign32 word;
    unalign64 wide;
};

_Static_assert(sizeof(unalign16) == 2, "unalign16 size");
_Static_assert(sizeof(unalign32) == 4, "unalign32 size");
_Static_assert(sizeof(unalign64) == 8, "unalign64 size");
_Static_assert(_Alignof(unalign16) == 1, "unalign16 alignment");
_Static_assert(_Alignof(unalign32) == 1, "unalign32 alignment");
_Static_assert(_Alignof(unalign64) == 1, "unalign64 alignment");
_Static_assert(sizeof(struct wire_values) == 15, "record size");
_Static_assert(_Alignof(struct wire_values) == 1, "record alignment");

static unalign32 read32(const void *pointer) {
    return *(const unalign32 *)pointer;
}

static void write32(void *pointer, unalign32 value) {
    *(unalign32 *)pointer = value;
}

static struct wire_values update(struct wire_values value) {
    value.tag += 1;
    value.half += 2;
    value.word += 3;
    value.wide += 4;
    return value;
}

int main(void) {
    struct wire_values value = {
        1,
        0x0203,
        0x04050607U,
        0x08090a0b0c0d0e0fUL,
    };
    unsigned char *base = (unsigned char *)&value;
    if ((unsigned char *)&value.half - base != 1) return 1;
    if ((unsigned char *)&value.word - base != 3) return 2;
    if ((unsigned char *)&value.wide - base != 7) return 3;

    write32(&value.word, 0x10203040U);
    if (read32(&value.word) != 0x10203040U) return 4;

    value = update(value);
    if (value.tag != 2) return 5;
    if (value.half != 0x0205) return 6;
    if (value.word != 0x10203043U) return 7;
    if (value.wide != 0x08090a0b0c0d0e13UL) return 8;
    return 67;
}
