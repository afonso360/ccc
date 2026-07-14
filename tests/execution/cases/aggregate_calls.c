struct Pair {
    long first;
    long second;
};

struct Mixed {
    long integer;
    double real;
};

struct Big {
    long first;
    long second;
    long third;
};

static int side_effects;

static int side(void) { return ++side_effects; }

static struct Pair shift_pair(struct Pair value) {
    value.first += 10;
    value.second += 20;
    return value;
}

static struct Mixed shift_mixed(struct Mixed value) {
    value.integer += 3;
    value.real += 4.0;
    return value;
}

static struct Big double_big(struct Big value) {
    value.first *= 2;
    value.second *= 2;
    value.third *= 2;
    return value;
}

static int comma_scalar(int value) { return (side(), value); }

static int comma_aggregate(struct Pair first, struct Pair second) {
    struct Pair shifted = shift_pair((side(), second));
    long member = (first, shifted).first;
    int scalar = comma_scalar(9);
    return side_effects == 2 && member == second.first + 10 && scalar == 9 ? 0
                                                                            : 100;
}

int main(void) {
    struct Pair pair;
    struct Mixed mixed;
    struct Big big;
    pair.first = 1;
    pair.second = 2;
    mixed.integer = 5;
    mixed.real = 6.0;
    big.first = 1;
    big.second = 2;
    big.third = 3;
    pair = shift_pair(pair);
    mixed = shift_mixed(mixed);
    big = double_big(big);
    return (int)(pair.first + pair.second + mixed.integer + mixed.real +
                 big.first + big.second + big.third) +
           comma_aggregate(pair, pair);
}
