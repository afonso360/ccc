struct Result {
    int items[3];
};

static struct Result make_result(int base) {
    struct Result result;
    result.items[0] = base;
    result.items[1] = base + 2;
    result.items[2] = base + 4;
    return result;
}

static struct Result choose_result(int choose_high) {
    return choose_high ? make_result(40) : make_result(1);
}

int main(void) {
    return choose_result(1).items[1];
}
