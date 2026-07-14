static const char greeting[] = "hello" " " "world";
static const char escapes[] = "\x41\101\n";
static const char exact_bound[2] = "xy";

int main(void) {
    const char *pointer = "abc";
    char mutable_copy[] = "xy";
    char exact_local[2] = "ab";

    if (sizeof(greeting) != 12 || greeting[5] != ' ' || greeting[11] != '\0')
        return 1;
    if (sizeof(escapes) != 4 || escapes[0] != '\x41'
        || escapes[1] != '\101' || escapes[2] != '\n')
        return 2;
    if (pointer[0] != 'a' || pointer[2] != 'c' || pointer[3] != '\0')
        return 3;
    mutable_copy[0] = 'z';
    if (mutable_copy[0] != 'z' || mutable_copy[1] != 'y')
        return 4;
    if (sizeof(exact_bound) != 2 || exact_bound[0] != 'x'
        || exact_bound[1] != 'y' || sizeof(exact_local) != 2
        || exact_local[0] != 'a' || exact_local[1] != 'b')
        return 5;
    return 46;
}
