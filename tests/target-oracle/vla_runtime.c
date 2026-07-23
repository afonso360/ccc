#ifdef __STDC_NO_VLA__
#error CCC must advertise complete variable-length array support
#endif

static int exercise(int rows, int columns) {
    int typedef_extent = columns;
    typedef int Row[typedef_extent++];
    if (typedef_extent != columns + 1) {
        return 1;
    }

    Row matrix[rows];
    matrix[rows - 1][columns - 1] = rows * 100 + columns;
    if (sizeof(Row) != (unsigned long)columns * sizeof(int)
        || sizeof(matrix) != (unsigned long)rows * columns * sizeof(int)
        || typedef_extent != columns + 1
        || matrix[rows - 1][columns - 1] != rows * 100 + columns) {
        return 2;
    }

    int type_name_extent = columns + 1;
    unsigned long dynamic_size =
        sizeof *(int (*)[type_name_extent++])(void *)0;
    unsigned long pointer_size = sizeof(int (*)[type_name_extent++]);
    unsigned long array_alignment = _Alignof(int[type_name_extent++]);
    if (dynamic_size != (unsigned long)(columns + 1) * sizeof(int)
        || pointer_size != sizeof(void *)
        || array_alignment != _Alignof(int)
        || type_name_extent != columns + 2) {
        return 3;
    }
    return 0;
}

int main(void) {
    for (int rows = 1; rows <= 16; ++rows) {
        if (exercise(rows, rows + 2) != 0) {
            return 1;
        }
    }
    return 0;
}
