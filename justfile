tmux:
    tmux new-session -A -s table-order

cashier-dev:
    cd apps/cashier && pnpm tauri dev

