static int test_barriers_and_layout_hints(void) {
    unsigned value = 41;
    asm("");
    asm("" : "+r"(value));
    asm volatile("" ::: "memory");
    asm(".p2align 3");
    asm(".p2align 4");
    asm(".p2align 5");
    asm(".p2align 6");
    asm("nop");
    return value != 41;
}

static int test_cpuid_and_rdtsc(void) {
    unsigned eax_only;
    unsigned eax_without_ebx;
    unsigned ecx_without_ebx;
    unsigned edx_without_ebx;
    unsigned eax_with_subleaf;
    unsigned ebx_with_subleaf;
    unsigned ecx_with_subleaf;
    unsigned low;
    unsigned high;

    asm("cpuid" : "=a"(eax_only) : "a"(0) : "ebx", "ecx", "edx");
    asm("cpuid"
        : "=a"(eax_without_ebx), "=c"(ecx_without_ebx), "=d"(edx_without_ebx)
        : "a"(0)
        : "ebx");
    asm("cpuid"
        : "=a"(eax_with_subleaf), "=b"(ebx_with_subleaf), "=c"(ecx_with_subleaf)
        : "a"(0), "c"(0)
        : "edx");

    if (eax_only != eax_without_ebx || eax_only != eax_with_subleaf) return 1;
    if (ecx_without_ebx != ecx_with_subleaf) return 2;
    if ((ebx_with_subleaf | ecx_with_subleaf | edx_without_ebx) == 0) return 3;

    asm volatile("rdtsc" : "=a"(low), "=d"(high));
    if ((((unsigned long long)high << 32) | low) == 0) return 4;
    return 0;
}

static int test_conditional_move(void) {
    unsigned long candidate_value = 11;
    unsigned long backup_value = 22;
    unsigned long *selected = &candidate_value;

    asm("cmp %1, %2\ncmova %3, %0\n"
        : "+r"(selected)
        : "r"(1U), "r"(2U), "r"(&backup_value));
    if (selected != &backup_value) return 1;

    selected = &candidate_value;
    asm("cmp %1, %2\ncmova %3, %0\n"
        : "+r"(selected)
        : "r"(2U), "r"(2U), "r"(&backup_value));
    if (selected != &candidate_value) return 2;
    return 0;
}

static int test_atomic_exchange(void) {
    unsigned long field = 10;
    unsigned long value = 20;
    unsigned long result = 0;
    unsigned long first = 1;
    unsigned long second = 2;
    unsigned long *pointer_field = &first;
    unsigned long *pointer_value = &second;

    asm volatile("lock; xchgq %0, %1" : "+q"(value), "+m"(field));
    if (field != 20 || value != 10) return 1;

    value = 30;
    asm volatile("lock; xchgq %1, %2"
                 : "=r"(result), "+q"(value), "+m"(field));
    if (field != 30 || value != 20 || result != 20) return 2;

    asm volatile("lock; xchgq %0, %1"
                 : "+q"(pointer_value), "+m"(pointer_field));
    if (pointer_field != &second || pointer_value != &first) return 3;
    return 0;
}

static int test_atomic_compare_exchange(void) {
    unsigned long field = 10;
    unsigned long expected = 10;
    unsigned long desired = 20;
    unsigned long original = 0;

    asm volatile("lock; cmpxchgq %2, %1"
                 : "=a"(original), "+m"(field)
                 : "q"(desired), "0"(expected));
    if (field != 20 || original != 10) return 1;

    expected = 99;
    desired = 30;
    asm volatile("lock; cmpxchgq %2, %1"
                 : "=a"(original), "+m"(field)
                 : "q"(desired), "0"(expected));
    if (field != 20 || original != 20) return 2;
    return 0;
}

int main(void) {
    int result;
    if ((result = test_barriers_and_layout_hints()) != 0) return 10 + result;
    if ((result = test_cpuid_and_rdtsc()) != 0) return 20 + result;
    if ((result = test_conditional_move()) != 0) return 30 + result;
    if ((result = test_atomic_exchange()) != 0) return 40 + result;
    if ((result = test_atomic_compare_exchange()) != 0) return 50 + result;
    return 0;
}
