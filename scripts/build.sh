#!/bin/bash
# builds the webui and the static genco binary with the webui embedded:
# target/x86_64-unknown-linux-musl/release/genco
set -euo pipefail
cd "$(dirname "$0")/.."

(cd webui && npm ci --silent && npx svelte-check --fail-on-warnings \
	--tsconfig ./tsconfig.json >/dev/null && npx vite build --logLevel warn)

cargo build --release --target x86_64-unknown-linux-musl \
	--manifest-path server/Cargo.toml
echo "built server/target/x86_64-unknown-linux-musl/release/genco"
