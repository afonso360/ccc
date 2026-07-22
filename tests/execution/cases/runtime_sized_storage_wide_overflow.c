int main(void) {
    volatile unsigned __int128 one = 1;
    unsigned __int128 extent = (one << 64) + 1;
    int values[extent];
    values[0] = 1;
    return values[0];
}
