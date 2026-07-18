extern long double ccc_darwin_long_double(long double);

int main(void) {
    return ccc_darwin_long_double(2.0L) == 2.5L ? 0 : 91;
}
