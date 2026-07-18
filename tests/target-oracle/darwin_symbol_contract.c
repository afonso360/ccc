int tentative_global;
int hidden_global __attribute__((visibility("hidden"))) = 3;

int hidden_native(void) __attribute__((visibility("hidden")));
int hidden_native(void) { return hidden_global; }

int hidden_variadic(int marker, ...) __attribute__((visibility("hidden")));
int hidden_variadic(int marker, ...) { return marker; }

static int internal_variadic(int marker, ...) { return marker + 1; }

int call_symbol_contract(void) {
    return tentative_global + hidden_native() + hidden_variadic(4) + internal_variadic(5);
}
