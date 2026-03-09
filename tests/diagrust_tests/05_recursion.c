#include <stdio.h>

int fibonacci(int n) {
    if (n <= 0) {
        return 0;
    }
    if (n == 1) {
        return 1;
    }
    return fibonacci(n - 1) + fibonacci(n - 2);
}

int factorial(int n) {
    if (n <= 1) {
        return 1;
    }
    return n * factorial(n - 1);
}

int main() {
    printf("Fibonacci sequence:\n");
    int i = 0;
    while (i < 10) {
        printf("fib(%d) = %d\n", i, fibonacci(i));
        i = i + 1;
    }

    printf("\nFactorials:\n");
    i = 1;
    while (i <= 10) {
        printf("%d! = %d\n", i, factorial(i));
        i = i + 1;
    }

    return 0;
}
