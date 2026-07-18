typedef unsigned long pthread_t;
typedef void *(*pthread_start_routine)(void *);

extern int pthread_create(
    pthread_t *thread,
    const void *attributes,
    pthread_start_routine start,
    void *argument);
extern int pthread_join(pthread_t thread, void **result);

_Thread_local int external_value = 11;

static int *block_value_address(void) {
    static _Thread_local int block_value = 17;
    return &block_value;
}

struct observation {
    int index;
    int initial_external;
    int initial_block;
    int final_external;
    int final_block;
    int *external_address;
    int *block_address;
};

static void *observe(void *argument) {
    struct observation *result = (struct observation *)argument;
    int *block_value = block_value_address();
    result->initial_external = external_value;
    result->initial_block = *block_value;
    result->external_address = &external_value;
    result->block_address = block_value;
    external_value = 30 + result->index;
    *block_value = 40 + result->index;
    result->final_external = external_value;
    result->final_block = *block_value;
    return argument;
}

int main(void) {
    enum { thread_count = 3 };
    pthread_t threads[thread_count];
    struct observation results[thread_count];
    int *main_external = &external_value;
    int *main_block = block_value_address();
    int index;

    external_value = 101;
    *main_block = 202;
    for (index = 0; index < thread_count; ++index) {
        results[index].index = index;
        if (pthread_create(&threads[index], (void *)0, observe, &results[index]) != 0)
            return 1;
    }
    for (index = 0; index < thread_count; ++index) {
        void *joined = (void *)0;
        if (pthread_join(threads[index], &joined) != 0) return 2;
        if (joined != &results[index]) return 3;
        if (results[index].initial_external != 11) return 4;
        if (results[index].initial_block != 17) return 5;
        if (results[index].final_external != 30 + index) return 6;
        if (results[index].final_block != 40 + index) return 7;
        if (results[index].external_address == main_external) return 8;
        if (results[index].block_address == main_block) return 9;
    }
    for (index = 0; index < thread_count; ++index) {
        int other;
        for (other = index + 1; other < thread_count; ++other) {
            if (results[index].external_address == results[other].external_address)
                return 10;
            if (results[index].block_address == results[other].block_address) return 11;
        }
    }
    if (external_value != 101 || *main_block != 202) return 12;
    return 66;
}
