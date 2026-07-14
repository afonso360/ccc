struct Entry {
    int value;
    const char *label;
};

static struct Entry entries[] = {
    {3, "alpha"},
    {4, "beta"},
    {5, "gamma"},
};
static int bias = 2;

static int add(int left, int right) { return left + right; }

static int multiply(int left, int right) { return left * right; }

int main(void) {
    int (*operations[2])(int, int) = {add, multiply};
    struct Entry *cursor = entries;
    int total = 0;
    int index;

    for (index = 0; index < 3; ++index)
        total = operations[index & 1](total, cursor[index].value + bias);

    switch (total) {
    case 37:
        total += 10;
        break;
    default:
        return 1;
    }

    if (cursor[0].label[0] != 'a' || cursor[1].label[0] != 'b'
        || cursor[2].label[0] != 'g')
        return 2;
    total += 6;
    goto done;

    return 3;
done:
    return total;
}
