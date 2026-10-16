#!/bin/sh
set -eu

: "${INFRAI_API_KEY:?Set INFRAI_API_KEY before running the demo}"
cargo run --quiet --bin dispatch_guard -- WO-204 en-route 1 false

