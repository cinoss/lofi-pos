# LoFi POS workspace — `just <command>`

default:
    @just --list

# ============ i18n ============

# Extract messages for ALL apps
i18n-extract: cashier-i18n-extract admin-i18n-extract web-i18n-extract

# Compile catalogs for ALL apps
i18n-compile: cashier-i18n-compile admin-i18n-compile web-i18n-compile

# Combo: extract + compile everywhere
i18n: i18n-extract i18n-compile

# Per-app
cashier-i18n-extract:
    pnpm --filter @lofi-pos/cashier i18n:extract

cashier-i18n-compile:
    pnpm --filter @lofi-pos/cashier i18n:compile

admin-i18n-extract:
    pnpm --filter @lofi-pos/admin i18n:extract

admin-i18n-compile:
    pnpm --filter @lofi-pos/admin i18n:compile

web-i18n-extract:
    pnpm --filter @lofi-pos/web i18n:extract

web-i18n-compile:
    pnpm --filter @lofi-pos/web i18n:compile

# ============ build / test ============

test:
    cd apps/cashier/src-tauri && cargo test

clippy:
    cd apps/cashier/src-tauri && cargo clippy --all-targets -- -D warnings

typecheck:
    pnpm -r typecheck

build:
    pnpm -r build

# ============ dev ============

tmux:
    tmux new-session -A -s table-order

# Build sidecar binaries used by the cashier (bouncer-mock)
build-sidecars:
    bash apps/cashier/src-tauri/scripts/build-sidecars.sh

# Run cashier in Tauri dev mode (bouncer-mock auto-spawned by Tauri)
cashier-dev: build-sidecars
    pnpm --filter @lofi-pos/cashier tauri:dev
