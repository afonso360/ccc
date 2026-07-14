int sink(int marker, ...);

int forward(char byte, unsigned short half, float real) {
    return sink(0, byte, half, real);
}
