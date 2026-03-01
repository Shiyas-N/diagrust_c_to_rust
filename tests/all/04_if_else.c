int max(int a, int b) {
    if (a > b) {
        return a;
    } else {
        return b;
    }
}

int sign(int x) {
    if (x > 0) {
        return 1;
    } else if (x < 0) {
        return -1;
    } else {
        return 0;
    }
}

int clamp(int val, int lo, int hi) {
    if (val < lo) {
        return lo;
    }
    if (val > hi) {
        return hi;
    }
    return val;
}
