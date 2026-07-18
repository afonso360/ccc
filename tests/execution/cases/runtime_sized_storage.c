typedef unsigned long pthread_t;
typedef void *(*pthread_start_routine)(void *);

extern int pthread_create(
    pthread_t *thread,
    const void *attributes,
    pthread_start_routine start,
    void *argument);
extern int pthread_join(pthread_t thread, void **result);

static int exercise_vla(int rows, int columns) {
    _Alignas(64) int matrix[rows][columns];
    if (((unsigned long)matrix & 63UL) != 0) {
        return 1;
    }
    for (int row = 0; row < rows; ++row) {
        for (int column = 0; column < columns; ++column) {
            matrix[row][column] = row * 100 + column;
        }
    }
    return matrix[rows - 1][columns - 1] != (rows - 1) * 100 + columns - 1;
}

static int recurse_with_vla(int depth) {
    int values[depth + 3];
    values[depth + 2] = depth * 7;
    if (depth == 0) {
        return values[2] != 0;
    }
    return values[depth + 2] != depth * 7 || recurse_with_vla(depth - 1);
}

static void *thread_with_vla(void *argument) {
    int seed = (int)(unsigned long)argument;
    for (int index = 1; index <= 20; ++index) {
        if (exercise_vla(seed + index, 3) != 0) {
            return (void *)1;
        }
    }
    return (void *)0;
}

int main(void) {
    int evaluated_once = 3;
    int once[evaluated_once++];
    once[2] = 9;
    if (evaluated_once != 4 || once[2] != 9) {
        return 1;
    }
    for (int count = 1; count <= 40; ++count) {
        int values[count];
        values[count - 1] = count * 2;
        if (values[count - 1] != count * 2) {
            return 2;
        }
    }
    if (exercise_vla(5, 7) != 0 || recurse_with_vla(12) != 0) {
        return 3;
    }

    pthread_t threads[4];
    for (int index = 0; index < 4; ++index) {
        if (pthread_create(
                &threads[index],
                (void *)0,
                thread_with_vla,
                (void *)(unsigned long)(index + 1)) != 0) {
            return 4;
        }
    }
    for (int index = 0; index < 4; ++index) {
        void *result = (void *)0;
        if (pthread_join(threads[index], &result) != 0 || result != (void *)0) {
            return 5;
        }
    }
    return 0;
}
