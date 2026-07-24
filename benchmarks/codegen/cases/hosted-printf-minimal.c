/* ccc-benchmark-family: hosted-printf */
/* ccc-benchmark-variant: minimal */
/* ccc-benchmark-scale: 1 */

extern int printf(const char *format, ...);

int main(void) {
    return printf("%s: %d\n", "hello from ccc", 42) < 0;
}
