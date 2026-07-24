/* ccc-benchmark-family: printf-variadic */
/* ccc-benchmark-scale: 1 */

extern int printf(const char *format, ...);

int main(void) {
    return printf("%s: %d\n", "hello from ccc", 42) < 0;
}
