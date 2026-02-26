int classify(int x) {
    if (x > 0) {
        if (x > 10) {
            return 2;
        } else {
            return 1;
        }
    } else {
        return 0;
    }
}
