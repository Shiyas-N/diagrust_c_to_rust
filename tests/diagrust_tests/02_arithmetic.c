#include <stdio.h>

int add(int a, int b) {
    return a + b;
}

int subtract(int a, int b) {
    return a - b;
}

int multiply(int a, int b) {
    return a * b;
}

float divide(float a, float b) {
    return a / b;
}

int main() {
    int x = 10;
    int y = 3;

    printf("x = %d, y = %d\n", x, y);
    printf("add:      %d\n", add(x, y));
    printf("subtract: %d\n", subtract(x, y));
    printf("multiply: %d\n", multiply(x, y));
    printf("divide:   %.2f\n", divide(x, y));

    return 0;
}
