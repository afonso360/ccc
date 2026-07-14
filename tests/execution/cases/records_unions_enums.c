enum Mode {
    MODE_OFF = 1,
    MODE_ON = 5,
    MODE_ALIAS = MODE_ON
};

struct Pair {
    int left;
    int right;
};

struct Node {
    struct Pair pair;
    enum Mode mode;
};

union Number {
    unsigned int unsigned_value;
    int signed_value;
};

static void copy_pair(struct Pair *destination, struct Pair *source) {
    *destination = *source;
}

int main(void) {
    struct Node original = {{19, 23}, MODE_ON};
    struct Node copy = original;
    union Number number;

    copy.pair.left = copy.pair.left + 1;
    copy_pair(&copy.pair, &copy.pair);
    if (original.pair.left != 19 || copy.pair.left != 20)
        return 1;
    if (copy.pair.right != 23 || copy.mode != MODE_ALIAS)
        return 2;

    number.signed_value = -7;
    if (number.signed_value != -7)
        return 3;
    number.unsigned_value = 29U;
    if (number.unsigned_value != 29U)
        return 4;
    return 43;
}
