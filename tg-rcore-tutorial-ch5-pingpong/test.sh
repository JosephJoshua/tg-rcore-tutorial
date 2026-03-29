#!/usr/bin/env bash
set -euo pipefail

GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m'

if ! command -v tg-rcore-tutorial-checker &> /dev/null; then
    echo -e "${YELLOW}Installing tg-rcore-tutorial-checker...${NC}"
    cargo install tg-rcore-tutorial-checker
fi

echo -e "${YELLOW}Building ch5-pingpong kernel (test mode)...${NC}"
cargo clean
export CHAPTER=-5
cargo build

KERNEL="target/riscv64gc-unknown-none-elf/debug/jsph-tg-rcore-tutorial-ch5-pingpong"

echo -e "${YELLOW}Running ch5-pingpong tests (90s timeout)...${NC}"
if timeout 90 qemu-system-riscv64 \
    -machine virt \
    -serial stdio \
    -bios none \
    -device virtio-gpu-device \
    -device virtio-keyboard-device \
    -display none \
    -kernel "$KERNEL" \
    2>&1 | tee /dev/stderr | tg-rcore-tutorial-checker --ch 5; then
    echo -e "${GREEN}PASS${NC}"
else
    echo -e "${RED}FAIL${NC}"
    exit 1
fi
