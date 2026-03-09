#include <stdio.h>

struct Stack {
    int data[50];
    int top;
};

void stack_init(struct Stack *s) {
    s->top = -1;
}

int stack_is_empty(struct Stack *s) {
    if (s->top == -1) {
        return 1;
    }
    return 0;
}

int stack_is_full(struct Stack *s) {
    if (s->top == 49) {
        return 1;
    }
    return 0;
}

void stack_push(struct Stack *s, int value) {
    if (stack_is_full(s) == 0) {
        s->top = s->top + 1;
        s->data[s->top] = value;
    }
}

int stack_pop(struct Stack *s) {
    if (stack_is_empty(s) == 0) {
        int val = s->data[s->top];
        s->top = s->top - 1;
        return val;
    }
    return -1;
}

int stack_peek(struct Stack *s) {
    if (stack_is_empty(s) == 0) {
        return s->data[s->top];
    }
    return -1;
}

int main() {
    struct Stack s;
    stack_init(&s);

    stack_push(&s, 10);
    stack_push(&s, 20);
    stack_push(&s, 30);
    stack_push(&s, 40);
    stack_push(&s, 50);

    printf("Top: %d\n", stack_peek(&s));

    printf("Popping: ");
    while (stack_is_empty(&s) == 0) {
        printf("%d ", stack_pop(&s));
    }
    printf("\n");

    printf("Stack empty: %d\n", stack_is_empty(&s));

    return 0;
}
