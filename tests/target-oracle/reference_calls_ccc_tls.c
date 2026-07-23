#include <pthread.h>

extern _Thread_local int ccc_tls_value;
extern int ccc_tls_read(void);
extern void ccc_tls_write(int value);

struct worker_state {
    int failed;
};

static void *worker(void *opaque) {
    struct worker_state *state = opaque;
    if (ccc_tls_value != 17 || ccc_tls_read() != 17) {
        state->failed = 1;
        return 0;
    }
    ccc_tls_write(42);
    if (ccc_tls_value != 42 || ccc_tls_read() != 42) {
        state->failed = 2;
    }
    return 0;
}

int main(void) {
    pthread_t thread;
    struct worker_state state = {0};
    ccc_tls_write(23);
    if (pthread_create(&thread, 0, worker, &state) != 0) {
        return 1;
    }
    if (pthread_join(thread, 0) != 0) {
        return 2;
    }
    if (state.failed != 0) {
        return 10 + state.failed;
    }
    return ccc_tls_value == 23 && ccc_tls_read() == 23 ? 0 : 3;
}
