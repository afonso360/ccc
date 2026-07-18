static int strings_equal(const char *left, const char *right) {
    while (*left && *left == *right) {
        left++;
        right++;
    }
    return *left == *right;
}

static int same_address(void) {
    const char *first = __func__;
    const char *second = __func__;
    return first == second && sizeof __func__ == sizeof "same_address";
}

static const char *reported_name(void) {
    return __func__;
}

static const char *static_capture(void) {
    static const char *const saved = __func__;
    return saved;
}

int main(void) {
    const char *reported = reported_name();
    const char *saved = static_capture();
    if (!same_address()) {
        return 1;
    }
    if (!strings_equal(reported, "reported_name") || reported_name() != reported) {
        return 2;
    }
    if (!strings_equal(saved, "static_capture") || static_capture() != saved) {
        return 3;
    }
    return 62;
}
