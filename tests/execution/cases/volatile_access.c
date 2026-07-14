static volatile int device_value = 3;

struct Pair {
    int left;
    int right;
};

static const volatile struct Pair device_pair = {13, 17};

static void copy_pair(struct Pair *destination,
                      const volatile struct Pair *source) {
    *destination = *source;
}

int main(void) {
    volatile int local_value = 5;
    int first_read = device_value;
    int ordinary = 9;
    int *ordinary_pointer = &ordinary;
    struct Pair pair = {0, 0};

    ordinary;
    *ordinary_pointer;
    pair.left;
    device_value;
    device_pair;
    copy_pair(&pair, &device_pair);

    device_value = first_read + 4;
    local_value = local_value + device_value;
    if (device_value != 7)
        return 1;
    if (local_value != 12)
        return 2;
    if (pair.left != 13 || pair.right != 17)
        return 3;
    return 50;
}
