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

echo -e "${YELLOW}Building ch4-tetris kernel...${NC}"
cargo build

KERNEL="target/riscv64gc-unknown-none-elf/debug/jsph-tg-rcore-tutorial-ch4-tetris"

echo -e "${YELLOW}Running ch4-tetris tests (60s timeout)...${NC}"
if timeout 60 qemu-system-riscv64 \
    -machine virt \
    -serial stdio \
    -bios none \
    -device virtio-gpu-device \
    -device virtio-keyboard-device \
    -display none \
    -kernel "$KERNEL" \
    2>&1 | tee /dev/stderr | tg-rcore-tutorial-checker --ch 4; then
    echo -e "${GREEN}PASS${NC}"
else
    echo -e "${RED}FAIL${NC}"
    exit 1
fi
