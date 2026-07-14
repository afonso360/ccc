int main(void) {
    int values[5] = {3, 5, 7, 11, 13};
    int matrix[2][3] = {{1, 2, 3}, {4, 5, 6}};
    int *cursor = values;
    int (*row)[3] = matrix;
    int *one_past = values + 5;
    _Bool has_values = values;

    cursor[2] = cursor[0] + cursor[1];
    if (values[2] != 8 || *(cursor + 3) != 11)
        return 1;
    if (one_past - cursor != 5 || &values[5] != one_past)
        return 2;
    if (row[1][2] != 6 || *(*(row + 1) + 1) != 5)
        return 3;
    if ((row + 1) - row != 1)
        return 4;
    if (has_values != 1 || (_Bool)(int *)0 != 0)
        return 5;
    return 42;
}
