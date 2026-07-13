int main(void) {
    int x = 7;
    {
        int x = 99;
        x = x + 1;
    }
    return x;
}
