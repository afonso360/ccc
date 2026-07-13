int main(void) {
    int x = 0;
    if (0 && (x = 1))
        return 1;
    if (1 || (x = 2))
        return x + 40;
    return 0;
}
