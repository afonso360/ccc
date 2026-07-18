static int value;
static int values[2];

int main(void) {
    char source[8] = "abcdef";
    char destination[8];

    ({ value; ; }) = 9;
    if (value != 9 || &({ value; }) != &value) {
        return 1;
    }
    if (({ int captured = value + 2; captured; }) != 11) {
        return 2;
    }
    if (({ values; }) != values) {
        return 3;
    }
    if (__func__ != __FUNCTION__ || __FUNCTION__ != __PRETTY_FUNCTION__) {
        return 4;
    }
    if (__builtin_memcpy(destination, source, 7) != destination ||
        destination[5] != 'f' || destination[6] != 0) {
        return 5;
    }
    if (__builtin_memmove(destination + 1, destination, 6) != destination + 1 ||
        destination[1] != 'a' || destination[6] != 'f') {
        return 6;
    }
    if (__builtin_memset(destination, 'A', 3) != destination ||
        destination[0] != 'A' || destination[2] != 'A') {
        return 7;
    }
    return 66;
}
