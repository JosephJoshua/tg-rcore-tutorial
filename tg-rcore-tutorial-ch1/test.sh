#!/bin/bash

# Build the kernel binary
cargo build 2>&1
BINARY="target/riscv64gc-unknown-none-elf/debug/tg-rcore-tutorial-ch1"

# Run QEMU in headless mode (no GPU window) for serial-only testing
OUTPUT=$(qemu-system-riscv64 \
    -machine virt \
    -display none \
    -serial stdio \
    -bios none \
    -kernel "$BINARY" 2>&1)

if echo "$OUTPUT" | grep -q "Hello, world!"; then
    echo "Test PASSED: Found 'Hello, world!' in output"
    exit 0
else
    echo "Test FAILED: 'Hello, world!' not found in output"
    echo "Actual output:"
    echo "$OUTPUT"
    exit 1
fi
