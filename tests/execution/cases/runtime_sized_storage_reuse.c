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
    }
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
    if (allocation_calls != 1 || release_calls != 1) {
        return 2;
    }
    return 66;
}
