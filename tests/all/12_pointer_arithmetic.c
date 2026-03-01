int read_value(int *p) {
    return *p;
}

void write_value(int *p, int val) {
    *p = val;
}

int double_deref(int **pp) {
    return **pp;
}
