int printf(const char *format, ...);

int main(void) {
    if (printf("ccc %d %.1f %s\n", 7, 2.5, "ok") != 13) {
        return 1;
    }
    return 0;
}
