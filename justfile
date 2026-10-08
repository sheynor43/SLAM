# Основные команды SLAM. Список: just --list

default: check

# fmt + clippy + тесты (то же, что в CI)
check: fmt-check clippy test

fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all -- --check

clippy:
    cargo clippy --workspace --all-targets -- -D warnings
    cargo clippy -p slam-engine --all-targets --features tracy -- -D warnings
    cargo clippy --workspace --all-targets --target x86_64-pc-windows-gnu -- -D warnings

# Тесты с коротким выводом
test *ARGS:
    ./scripts/test-summary.sh {{ARGS}}

# Лицензии и уязвимости зависимостей
deny:
    cargo deny check

bench *ARGS:
    cargo bench --workspace {{ARGS}}

run *ARGS:
    cargo run -p slam-app --release -- {{ARGS}}

# Корпусы реплеев (путь к корпусу: SLAM_CORPUS, по умолчанию ../slam-corpus)
corpus *ARGS:
    cargo test -p slam-osu --release --test corpus -- --ignored {{ARGS}}
