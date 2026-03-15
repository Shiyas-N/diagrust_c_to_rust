// Showcase: Mutability Repair
// Triggers: LoopVarMut, AddMut, CompoundAssign, MutBindingForMutRef, MutCallArgs
// The repair loop must add `mut` to bindings, convert x = x + 1 to x += 1,
// and upgrade &var to &mut var for pointer parameters.
#include <stdio.h>

void increment(int *val) {
    *val = *val + 1;
}

void accumulate(int arr[], int n, int *total) {
    int i;
    for (i = 0; i < n; i++) {
        *total = *total + arr[i];
    }
}

int main() {
    int x = 10;
    increment(&x);
    printf("After increment: %d\n", x);

    int nums[5];
    nums[0] = 10;
    nums[1] = 20;
    nums[2] = 30;
    nums[3] = 40;
    nums[4] = 50;

    int sum = 0;
    accumulate(nums, 5, &sum);
    printf("Sum: %d\n", sum);

    // Loop with compound assignment pattern
    int count = 0;
    int i;
    for (i = 0; i < 10; i++) {
        count = count + i;
    }
    printf("Count: %d\n", count);

    // Nested mutation
    int a = 5;
    int b = 3;
    a = a * b;
    b = b + a;
    printf("a=%d, b=%d\n", a, b);

    return 0;
}
