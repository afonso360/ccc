int variadic(const char *format, ...);
int function(void) {
    return variadic("x", 1);
}
