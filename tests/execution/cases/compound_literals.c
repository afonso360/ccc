struct Pair {
    int left;
    int right;
};

struct Holder {
    struct Pair pair;
};

struct PointerHolder {
    int *pointer;
};

struct Pair *file_first = &(struct Pair){.left = 10, .right = 11};
struct Pair *file_second = &(struct Pair){.left = 10, .right = 11};
const struct Pair *file_qualified =
    &(const struct Pair){.left = 12, .right = 13};
int *file_values = (int[]){14, 15, 16};
struct PointerHolder *file_nested =
    &(struct PointerHolder){.pointer = &(int){17}};
struct PointerHolder file_copied =
    (struct PointerHolder){.pointer = &(int){19}};

int main(void) {
    struct Pair first = (struct Pair){.left = 20, .right = 22};
    struct Pair *second = &(struct Pair){.left = 5, .right = 6};
    int *saved = 0;
    int sizes[sizeof((int[]){1, 2, 3}) / sizeof(int) == 3 ? 1 : -1];

    if (file_first == file_second || file_first->left != 10 || file_second->right != 11) {
        return 6;
    }
    file_first->left = 18;
    if (file_first->left != 18 || file_second->left != 10) {
        return 7;
    }
    if (file_qualified->left != 12 || file_qualified->right != 13) {
        return 8;
    }
    if (file_values[0] != 14 || file_values[1] != 15 || file_values[2] != 16) {
        return 9;
    }
    if (*file_nested->pointer != 17) {
        return 10;
    }
    if (*file_copied.pointer != 19) {
        return 11;
    }

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
