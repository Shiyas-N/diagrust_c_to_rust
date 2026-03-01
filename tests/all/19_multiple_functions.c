int square(int x) {
    return x * x;
}

int cube(int x) {
    return x * square(x);
}

int sum_of_squares(int a, int b, int c) {
    return square(a) + square(b) + square(c);
}

int hypotenuse_sq(int a, int b) {
    return square(a) + square(b);
}
