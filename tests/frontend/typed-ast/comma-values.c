struct Pair {
    int value;
};

int side(void);
int take(struct Pair value);

int scalar(int value) { return (side(), value); }

int aggregate(struct Pair first, struct Pair second) {
    return take((side(), second)) + (first, second).value;
}
