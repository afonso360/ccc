void *realloc(void *previous, unsigned long size) {
    (void)previous;
    (void)size;
    return (void *)0;
}

void free(void *address) {
    (void)address;
}

int main(void) {
    volatile int extent = 8;
    int values[extent];
    values[extent - 1] = 1;
    return values[extent - 1];
}
