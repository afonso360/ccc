static int dispatch(int opcode) {
    static const void *const table[3] = {&&zero, &&one, &&two};

    if ((unsigned)opcode >= 3) {
        return 90;
    }
    goto *table[opcode];

zero:
    return 10;
one:
    return 20;
two:
    return 30;
}

int main(void) {
    void *first = &&local;
    void *same = &&local;
    void *automatic[2] = {&&wrong, &&through_table};

    if (first != same) {
        return 1;
    }
    goto *&&direct_target;

wrong:
    return 2;
direct_target:
    goto *automatic[1];
through_table:
    goto *first;
local:
    return dispatch(0) + dispatch(1) + dispatch(2) == 60 && dispatch(3) == 90 ? 57 : 3;
}
