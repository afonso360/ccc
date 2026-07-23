typedef unsigned long pthread_t;
typedef void *(*pthread_start_routine)(void *);

extern int pthread_create(
    pthread_t *thread,
    const void *attributes,
    pthread_start_routine start,
    void *argument);
extern int pthread_join(pthread_t thread, void **result);

static int sizeof_operand_calls;
static int left_bound_calls;
static int right_bound_calls;

static int observe_sizeof_operand(void) {
    ++sizeof_operand_calls;
    return 0;
}

static int next_left_bound(void) {
    ++left_bound_calls;
    return 5;
}

static int next_right_bound(void) {
    ++right_bound_calls;
    return 5;
}

static int exercise_conditional_vla_stride(int choose) {
    int left[5];
    int right[5];
    left_bound_calls = 0;
    right_bound_calls = 0;
    char *end = (char *)((choose
        ? (int (*)[next_left_bound()])left
        : (int (*)[next_right_bound()])right) + 1);
    char *start = (char *)(choose ? left : right);
    unsigned long stride = (unsigned long)(end - start);
    return stride != 5 * sizeof(int)
        || left_bound_calls != (choose != 0)
        || right_bound_calls != (choose == 0);
}

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
    int typedef_extent = 3;
    typedef int Vector[typedef_extent++];
    Vector vector;
    vector[2] = 11;
    if (typedef_extent != 4 || sizeof(Vector) != 3 * sizeof(int)
        || sizeof(vector) != 3 * sizeof(int) || vector[2] != 11) {
        return 6;
    }
    int type_name_extent = 4;
    unsigned long dynamic_size = sizeof *(int (*)[type_name_extent++])(void *)0;
    unsigned long pointer_size = sizeof(int (*)[type_name_extent++]);
    unsigned long array_alignment = _Alignof(int[type_name_extent++]);
    if (dynamic_size != 4 * sizeof(int) || pointer_size != sizeof(void *)
        || array_alignment != _Alignof(int) || type_name_extent != 5) {
        return 7;
    }
    int sizeof_operand_side_effects = 0;
    int sizeof_operand_extent = 4;
    int sizeof_operand_backing[4];
    int (*sizeof_operand_pointer)[sizeof_operand_extent]
        = (int (*)[sizeof_operand_extent])&sizeof_operand_backing;
    unsigned long evaluated_size = sizeof *((
        sizeof_operand_side_effects++,
        observe_sizeof_operand(),
        sizeof_operand_pointer));
    if (evaluated_size != sizeof(sizeof_operand_backing)
        || sizeof_operand_side_effects != 1 || sizeof_operand_calls != 1) {
        return 8;
    }
    if (exercise_conditional_vla_stride(1) != 0
        || exercise_conditional_vla_stride(0) != 0) {
        return 9;
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
