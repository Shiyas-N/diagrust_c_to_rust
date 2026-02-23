#!/bin/bash
echo "🧪 Testing all C samples..."
for file in tests/c_samples/*.c; do
    echo "Testing $(basename $file)..."
    ./target/release/diagrust "$file" -v
    echo ""
done
echo "✅ All tests complete!"
