static int protected_evaluations;

static int protected_operand(void) {
    protected_evaluations++;
    return 0;
}

static void first_callback(void) {}
static void second_callback(void) {}

typedef void (*callback)(void);

_Static_assert(
    sizeof(__sync_bool_compare_and_swap((int *)0, 0, 1)) == sizeof(_Bool),
    "Boolean compare-and-swap must have _Bool type");

int main(void) {
    signed char narrow = 3;
    unsigned short medium = 10;
    volatile int ordinary = 20;
    unsigned long long wide = 100;
    void *pointer = (void *)0;
    callback selected = first_callback;

    if (__sync_fetch_and_add(&narrow, 2) != 3 || narrow != 5) return 1;
    if (__sync_add_and_fetch(&medium, 7) != 17 || medium != 17) return 2;
    if (__sync_sub_and_fetch(&ordinary, 3, protected_operand()) != 17) return 3;
    if (protected_evaluations != 0) return 4;
    if (__sync_fetch_and_add(&wide, 23, __sync_synchronize) != 100 || wide != 123)
        return 5;

    if (!__sync_bool_compare_and_swap(&ordinary, 17, 31) || ordinary != 31) return 6;
    if (__sync_bool_compare_and_swap(&ordinary, 17, 99) || ordinary != 31) return 7;
    if (__sync_val_compare_and_swap(&ordinary, 31, 41) != 31 || ordinary != 41)
        return 8;
    if (__sync_val_compare_and_swap(&ordinary, 31, 99) != 41 || ordinary != 41)
        return 9;

    if (__sync_add_and_fetch(&pointer, (void *)1) != (void *)1) return 10;
    if (__sync_lock_test_and_set(&pointer, (void *)32) != (void *)1) return 11;
    if (pointer != (void *)32) return 12;

    if (__sync_val_compare_and_swap(&selected, first_callback, second_callback)
        != first_callback)
        return 13;
    if (selected != second_callback) return 14;

    selected = (callback)0;
    if (!__sync_bool_compare_and_swap(
            &selected, ((void *)0), first_callback))
        return 15;
    if (selected != first_callback) return 16;

    pointer = (void *)0;
    if (__sync_add_and_fetch(&pointer, 1) != (void *)1) return 17;

    return 60;
}
