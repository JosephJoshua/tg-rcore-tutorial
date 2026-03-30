#!/bin/bash
set -e

GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[0;33m'
NC='\033[0m'

ensure_tg_checker() {
    if ! command -v tg-rcore-tutorial-checker &> /dev/null; then
        echo -e "${YELLOW}tg-rcore-tutorial-checker not installed, installing...${NC}"
        if cargo install tg-rcore-tutorial-checker; then
            echo -e "${GREEN}✓ tg-rcore-tutorial-checker installed${NC}"
        else
            echo -e "${RED}✗ tg-rcore-tutorial-checker install failed${NC}"
            exit 1
        fi
    fi
}

ensure_tg_checker
set -o pipefail

run_base() {
    echo "Running ch7-pacman base tests..."
    cargo clean
    export CHAPTER=-7
    echo -e "${YELLOW}────────── cargo run output ──────────${NC}"

    if cargo run 2>&1 | tee /dev/stderr | tg-rcore-tutorial-checker --ch 7; then
        echo ""
        echo -e "${YELLOW}────────── test results ──────────${NC}"
        echo -e "${GREEN}✓ ch7-pacman base tests passed${NC}"
        cargo clean
        return 0
    else
        echo ""
        echo -e "${YELLOW}────────── test results ──────────${NC}"
        echo -e "${RED}✗ ch7-pacman base tests failed${NC}"
        cargo clean
        return 1
    fi
}

case "${1:-base}" in
    base|all)
        run_base
        ;;
    *)
        echo "Usage: $0 [base|all]"
        exit 1
        ;;
esac
