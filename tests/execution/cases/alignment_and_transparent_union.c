struct IPv4;
struct IPv6;

typedef union {
    int *integer;
    const unsigned int *unsigned_integer;
    struct IPv4 *ipv4;
    const struct IPv6 *ipv6;
} PointerArgument __attribute__((__transparent_union__));

struct AlignedState {
    char tag;
    _Alignas(64) unsigned long lanes[8];
};

_Alignas(64) static unsigned char file_storage[1];
static struct AlignedState state;

static int read_pointer(PointerArgument argument) {
    return *argument.integer;
}

int main(void) {
    _Alignas(64) unsigned long automatic_storage[8] = {0};
    _Alignas(64) static unsigned char block_storage[1];
    int value = 65;
    PointerArgument wrapped = {.integer = &value};

    if (((unsigned long)file_storage & 63UL) != 0)
        return 1;
    if (((unsigned long)block_storage & 63UL) != 0)
        return 2;
    if (((unsigned long)automatic_storage & 63UL) != 0)
        return 3;
    if (((unsigned long)&state.lanes & 63UL) != 0)
        return 4;
    if (sizeof(struct AlignedState) != 128UL)
        return 5;
    if (read_pointer(&value) != 65)
        return 6;
    if (read_pointer(wrapped) != 65)
        return 7;
    return value;
}
