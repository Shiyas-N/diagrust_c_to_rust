int compute_type(int type, int mod, int match) {
    int result = type + mod;
    result = result * match;
    return result;
}
