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

# Удалить сборочные артефакты из target/, кроме базы бенчмарков (target/criterion)
clean:
    find target -mindepth 1 -maxdepth 1 ! -name criterion -exec rm -rf {} +

[positional-arguments]
run *ARGS:
    cargo run -p slam-app --release -- "$@"

# Тестовый тон 440 Гц через аудио-HAL: just tone [--backend ИМЯ] [секунды] [кадры_буфера] [устройство]
tone *ARGS:
    cargo run -p slam-audio --release --example tone -- {{ARGS}}

# Хитсаунд через микшер по Enter: just hitsound [файл] [кадры_буфера]
hitsound *ARGS:
    cargo run -p slam-audio --release --example hitsound -- {{ARGS}}

# Корпусы реплеев; FILTER - подстрока пути реплея (путь к корпусу: SLAM_CORPUS, по умолчанию ../slam-corpus)
corpus FILTER="":
    SLAM_CORPUS_FILTER={{quote(FILTER)}} cargo test -p slam-osu --release --test corpus -- --ignored --nocapture

# Проверка локализации встроенных языков: just i18n check
i18n *ARGS:
    cargo run -p slam-app -- i18n {{ARGS}}
