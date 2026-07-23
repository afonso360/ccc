struct Bits {
    unsigned flag : 3;
};

static int controlling_evaluations;
static int selected_evaluations;
static int unselected_evaluations;

static int controlling(void) {
    ++controlling_evaluations;
    return 0;
}

static int selected(void) {
    ++selected_evaluations;
    return 7;
}

static int unselected(void) {
    ++unselected_evaluations;
    return 99;
}

_Static_assert(_Generic(*(const int *)0, int: 1, default: 0), "lvalue conversion");
_Static_assert(_Generic("abc", char *: 1, default: 0), "array conversion");
_Static_assert(
    _Generic(selected, int (*)(void): 1, default: 0),
    "function conversion"
);
_Static_assert(sizeof(_Generic(0, int: "abc", default: "x")) == 4, "array result");
_Static_assert(_Generic(controlling(), int: 42, default: 0) == 42, "constant result");

int main(void) {
    struct Bits bits = {0};
    int value = 0;

    _Generic(controlling(), int: value, default: value) = 34;
    _Generic(controlling(), int: bits.flag, default: bits.flag) = 3;
    int result = _Generic(controlling(), int: selected(), default: unselected());

    if (controlling_evaluations != 0) {
        return 1;
    }
    if (selected_evaluations != 1 || unselected_evaluations != 0) {
        return 2;
    }
    if (value != 34 || bits.flag != 3 || result != 7) {
        return 3;
    }
    return value + bits.flag + result;
}
