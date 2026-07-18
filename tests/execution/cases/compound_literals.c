struct Pair {
    int left;
    int right;
};

struct Holder {
    struct Pair pair;
};

int main(void) {
    struct Pair first = (struct Pair){.left = 20, .right = 22};
    struct Pair *second = &(struct Pair){.left = 5, .right = 6};
    int *saved = 0;
    int sizes[sizeof((int[]){1, 2, 3}) / sizeof(int) == 3 ? 1 : -1];

    for (int index = 0; index < 2; ++index) {
        int *current = &(int){index + 4};
        if (index == 0) {
            saved = current;
        } else if (current != saved || *current != 5) {
            return 1;
        }
    }
    if ((struct Pair){.left = 8, .right = 9}.right != 9) {
        return 3;
    }
    if ((struct Holder){.pair = {.left = 7, .right = 6}}.pair.left != 7) {
        return 4;
    }
    {
        volatile int *qualified = &(volatile int){3};
        *qualified = 4;
        if (*qualified != 4) {
            return 5;
        }
    }
    if (sizes + 1 == sizes) {
        return 2;
    }
    return first.left + first.right + second->left + second->right + *saved;
}
