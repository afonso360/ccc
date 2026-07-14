struct Inner {
    unsigned prefix;
    signed value : 6;
};

struct Outer {
    struct Inner inner;
    unsigned tail;
};

static struct Outer make_outer(void) {
    struct Outer result;
    result.inner.prefix = 0x12345678U;
    result.inner.value = -5;
    result.tail = 0x87654321U;
    return result;
}

int main(void) {
    return make_outer().inner.value + 42;
}
