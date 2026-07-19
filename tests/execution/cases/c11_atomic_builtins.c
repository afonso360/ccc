#include <stdatomic.h>

static int order_evaluations;

static memory_order weaker_order(void) {
    ++order_evaluations;
    return memory_order_relaxed;
}

int main(void) {
    atomic_schar narrow = ATOMIC_VAR_INIT(3);
    atomic_short medium = ATOMIC_VAR_INIT(10);
    atomic_int ordinary = ATOMIC_VAR_INIT(20);
    atomic_llong wide = ATOMIC_VAR_INIT(100);
    atomic_flag flag = ATOMIC_FLAG_INIT;
    int first = 1;
    int second = 2;
    int elements[3];
    _Atomic(int *) pointer = &first;
    _Atomic(int *) cursor = elements;
    atomic_int arithmetic = ATOMIC_VAR_INIT(2);
    int expected;

    if (atomic_load(&narrow) != 3) return 1;
    atomic_store_explicit(&narrow, 5, weaker_order());
    if (atomic_load_explicit(&narrow, weaker_order()) != 5) return 2;
    if (order_evaluations != 2) return 3;

    if (atomic_fetch_add(&medium, 7) != 10 || atomic_load(&medium) != 17)
        return 4;
    if (__atomic_add_fetch(&ordinary, 3, __ATOMIC_ACQUIRE) != 23)
        return 5;
    if (__atomic_sub_fetch(&ordinary, 2, __ATOMIC_RELEASE) != 21)
        return 6;
    if (__atomic_fetch_or(&ordinary, 8, __ATOMIC_RELAXED) != 21)
        return 7;
    if (__atomic_fetch_xor(&ordinary, 1, __ATOMIC_ACQ_REL) != 29)
        return 8;
    if (__atomic_and_fetch(&ordinary, 30, __ATOMIC_SEQ_CST) != 28)
        return 9;
    if (atomic_exchange(&wide, 123) != 100 || atomic_load(&wide) != 123)
        return 10;

    expected = 28;
    if (!atomic_compare_exchange_strong(&ordinary, &expected, 41)) return 11;
    if (atomic_load(&ordinary) != 41 || expected != 28) return 12;
    expected = 28;
    if (atomic_compare_exchange_weak_explicit(
            &ordinary, &expected, 99,
            memory_order_relaxed, memory_order_relaxed))
        return 13;
    if (expected != 41 || atomic_load(&ordinary) != 41) return 14;

    if (atomic_load(&pointer) != &first) return 15;
    if (atomic_exchange(&pointer, &second) != &first) return 16;
    if (atomic_load(&pointer) != &second) return 17;

    if (atomic_flag_test_and_set(&flag)) return 18;
    if (!atomic_flag_test_and_set_explicit(&flag, memory_order_relaxed)) return 19;
    atomic_flag_clear(&flag);
    if (atomic_flag_test_and_set(&flag)) return 20;

    ordinary = 50;
    if (ordinary != 50) return 21;
    atomic_thread_fence(memory_order_acquire);
    atomic_signal_fence(memory_order_release);
    if (!atomic_is_lock_free(&ordinary)) return 22;
    if (ATOMIC_INT_LOCK_FREE != 2 || ATOMIC_POINTER_LOCK_FREE != 2) return 23;

    if ((arithmetic += 3) != 5) return 24;
    if ((arithmetic &= 6) != 4) return 25;
    if ((arithmetic |= 1) != 5) return 26;
    if ((arithmetic ^= 7) != 2) return 27;
    if (arithmetic++ != 2 || ++arithmetic != 4) return 28;
    if (cursor++ != elements) return 29;
    if (++cursor != elements + 2) return 30;
    if ((cursor -= 2) != elements) return 31;

    return 68;
}
