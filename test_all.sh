#!/bin/bash

echo "🧪 Testing all C files inside tests/ recursively..."

PASS_COUNT=0
FAIL_COUNT=0

while IFS= read -r file; do
    echo "----------------------------------------"
    echo "Testing: $file"

    ./target/release/diagrust "$file" -v
    EXIT_CODE=$?

    if [[ "$file" == *"validation/invalid/"* ]]; then
        if [ $EXIT_CODE -ne 0 ]; then
            echo "✅ Correctly detected invalid file"
            ((PASS_COUNT++))
        else
            echo "❌ Invalid file passed unexpectedly"
            ((FAIL_COUNT++))
        fi
    else
        if [ $EXIT_CODE -eq 0 ]; then
            echo "✅ Passed"
            ((PASS_COUNT++))
        else
            echo "❌ Failed"
            ((FAIL_COUNT++))
        fi
    fi

    echo ""
done < <(find tests -type f -name "*.c")

echo "========================================"
echo "🎯 Test Summary:"
echo "Passed: $PASS_COUNT"
echo "Failed: $FAIL_COUNT"
echo "========================================"