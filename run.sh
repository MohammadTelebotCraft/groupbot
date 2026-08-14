#!/bin/sh
set -a; . "$(dirname "$0")/.env"; set +a
LOG=${LOG:-warn} exec cargo run --release "$@"
