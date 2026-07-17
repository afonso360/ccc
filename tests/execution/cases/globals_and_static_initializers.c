struct Pair {
    int left;
    int right;
};

int global_counter = 5;
int *global_pointer = &global_counter;
static int static_values[4] = {[3] = 13, [0] = 7, [2] = 11, [1] = 5};
static struct Pair static_pair = {.right = 19, .left = 17};
static int zero_initialized;
static int address_values[4] = {3, 5, 7, 11};
static int *offset_pointer = &address_values[2];
static const char name_storage[] = "named";
static const char *name_pointer = name_storage;
static unsigned long long high_bit = (unsigned long long)1 << 40;

static int next_call_value(void) {
    static int value = 2;
    value = value + 3;
    return value;
}

int main(void) {
    int first_call;
    int second_call;

    if (global_pointer != &global_counter || *global_pointer != 5)
        return 1;
    if (static_values[0] != 7 || static_values[1] != 5
        || static_values[2] != 11 || static_values[3] != 13)
        return 2;
    if (static_pair.left != 17 || static_pair.right != 19)
        return 3;
    if (zero_initialized != 0)
        return 4;
    if (offset_pointer != &address_values[2] || *offset_pointer != 7)
        return 7;
    if (name_pointer != name_storage || name_pointer[4] != 'd')
        return 8;
    if (high_bit != 1099511627776ULL)
        return 9;

    {
        char value = 'x';
        char *plain = &value;
        const char *qualified = &value;
        const char *selected = global_counter ? plain : qualified;
        if (plain != qualified || qualified != plain || selected != &value)
            return 10;
    }

    first_call = next_call_value();
    second_call = next_call_value();
    if (first_call != 5 || second_call != 8)
        return 5;
    global_counter = global_counter + 4;
    if (*global_pointer != 9)
        return 6;
    return 45;
}
