struct Packet {
    unsigned length;
    int values[];
};

void *malloc(unsigned long size);
void free(void *pointer);

int main(void) {
    struct Packet *packet = malloc(sizeof(struct Packet) + 3 * sizeof(int));
    int result;

    if (!packet) {
        return 1;
    }

    packet->length = 3;
    packet->values[0] = 11;
    packet->values[1] = 17;
    packet->values[2] = 24;
    result = sizeof(struct Packet) + packet->length + packet->values[0] +
             packet->values[1] + packet->values[2];
    free(packet);
    return result;
}
