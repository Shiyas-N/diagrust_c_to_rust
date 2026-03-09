#include <stdio.h>
#include <stdlib.h>

struct Node {
    int value;
    int next_index;
};

int stack_top = 0;
struct Node pool[100];

int new_node(int value) {
    pool[stack_top].value = value;
    pool[stack_top].next_index = -1;
    int idx = stack_top;
    stack_top = stack_top + 1;
    return idx;
}

int push_front(int head, int value) {
    int new_idx = new_node(value);
    pool[new_idx].next_index = head;
    return new_idx;
}

void print_list(int head) {
    int current = head;
    while (current != -1) {
        printf("%d", pool[current].value);
        if (pool[current].next_index != -1) {
            printf(" -> ");
        }
        current = pool[current].next_index;
    }
    printf("\n");
}

int list_sum(int head) {
    int sum = 0;
    int current = head;
    while (current != -1) {
        sum = sum + pool[current].value;
        current = pool[current].next_index;
    }
    return sum;
}

int main() {
    int head = -1;

    head = push_front(head, 10);
    head = push_front(head, 20);
    head = push_front(head, 30);
    head = push_front(head, 40);
    head = push_front(head, 50);

    printf("List: ");
    print_list(head);

    printf("Sum: %d\n", list_sum(head));

    return 0;
}
