typedef unsigned long pthread_t;
typedef void *(*pthread_start_routine)(void *);

extern int pthread_create(
    pthread_t *thread,
    const void *attributes,
    pthread_start_routine start,
    void *argument);
extern int pthread_join(pthread_t thread, void **result);

enum { thread_count = 4, iterations = 10000 };

static volatile int fetch_counter;
static volatile int new_counter;

static void *increment(void *argument) {
    int index;
    for (index = 0; index < iterations; index++) {
        __sync_fetch_and_add(&fetch_counter, 1);
        __sync_add_and_fetch(&new_counter, 1);
    }
    return argument;
}

int main(void) {
    pthread_t threads[thread_count];
    int index;

    for (index = 0; index < thread_count; index++) {
        void *argument = (void *)(unsigned long)(index + 1);
        if (pthread_create(&threads[index], (void *)0, increment, argument) != 0)
            return 1;
    }
    for (index = 0; index < thread_count; index++) {
        void *result = (void *)0;
        if (pthread_join(threads[index], &result) != 0) return 2;
        if (result != (void *)(unsigned long)(index + 1)) return 3;
    }
    if (fetch_counter != thread_count * iterations) return 4;
    if (new_counter != thread_count * iterations) return 5;
    return 64;
}
