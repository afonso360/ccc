static unsigned char allocation[1024];
static int allocation_calls;
static int release_calls;

void *realloc(void *previous, unsigned long size) {
    (void)previous;
    ++allocation_calls;
    if (size > sizeof allocation) {
        return (void *)0;
    }
    return allocation;
}

void free(void *address) {
    if (address != (void *)0) {
        ++release_calls;
        for (unsigned long index = 0; index < sizeof allocation; ++index) {
            allocation[index] = 0xa5;
        }
    }
}

struct register_result {
    long first;
    long second;
};

struct memory_result {
    long first;
    long second;
    long third;
};

static struct register_result return_register_result(int extent) {
    struct register_result values[extent];
    values[extent - 1].first = 17;
    values[extent - 1].second = 29;
    return values[extent - 1];
}

static struct memory_result return_memory_result(int extent) {
    struct memory_result values[extent];
    values[extent - 1].first = 31;
    values[extent - 1].second = 37;
    values[extent - 1].third = 41;
    return values[extent - 1];
}

static int exercise(void) {
    for (int extent = 128; extent >= 1; --extent) {
        int values[extent];
        values[extent - 1] = extent;
        if (values[extent - 1] != extent) {
            return 1;
        }
    }
    return allocation_calls != 1;
}

int main(void) {
    if (exercise() != 0) {
        return 1;
    }
    struct register_result registers = return_register_result(3);
    if (registers.first != 17 || registers.second != 29) {
        return 2;
    }
    struct memory_result memory = return_memory_result(3);
    if (memory.first != 31 || memory.second != 37 || memory.third != 41) {
        return 3;
    }
    if (allocation_calls != 3 || release_calls != 3) {
        return 4;
    }
    return 66;
}
