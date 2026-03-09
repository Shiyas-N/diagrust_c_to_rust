#include <stdio.h>

int account_count = 0;

struct Account {
    int id;
    int balance;
    int is_active;
};

struct Account create_account(int initial_balance) {
    struct Account acc;
    account_count = account_count + 1;
    acc.id = account_count;
    acc.balance = initial_balance;
    acc.is_active = 1;
    return acc;
}

int deposit(struct Account *acc, int amount) {
    if (acc->is_active == 0) {
        return 0;
    }
    if (amount <= 0) {
        return 0;
    }
    acc->balance = acc->balance + amount;
    return 1;
}

int withdraw(struct Account *acc, int amount) {
    if (acc->is_active == 0) {
        return 0;
    }
    if (amount <= 0) {
        return 0;
    }
    if (acc->balance < amount) {
        return 0;
    }
    acc->balance = acc->balance - amount;
    return 1;
}

int transfer(struct Account *from, struct Account *to, int amount) {
    int ok = withdraw(from, amount);
    if (ok == 1) {
        deposit(to, amount);
        return 1;
    }
    return 0;
}

void print_account(struct Account *acc) {
    printf("Account #%d | Balance: %d | Active: %d\n",
        acc->id, acc->balance, acc->is_active);
}

int main() {
    struct Account alice = create_account(1000);
    struct Account bob   = create_account(500);
    struct Account carol = create_account(250);

    printf("=== Initial State ===\n");
    print_account(&alice);
    print_account(&bob);
    print_account(&carol);

    deposit(&alice, 200);
    withdraw(&bob, 100);
    transfer(&alice, &carol, 300);

    printf("\n=== After Transactions ===\n");
    print_account(&alice);
    print_account(&bob);
    print_account(&carol);

    int failed = withdraw(&bob, 1000);
    printf("\nOverdraft attempt: %d (0 = failed)\n", failed);

    printf("Total accounts created: %d\n", account_count);

    return 0;
}
