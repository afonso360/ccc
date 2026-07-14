typedef int (*binary_operation)(int, int);

static int add(int left, int right) { return left + right; }

static int subtract(int left, int right) { return left - right; }

static int apply(binary_operation operation, int left, int right) {
    return operation(left, right);
}

int main(void) {
    binary_operation operations[2] = {add, subtract};
    int sum = apply(operations[0], 19, 23);
    int difference = (*operations[1])(50, 8);

    if (sum != 42 || difference != 42)
        return 1;
    return 48;
}
