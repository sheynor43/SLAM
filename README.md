# SLAM — Stable Lazer Always Mythical

Самостоятельный клиент для режима osu!standard с открытым исходным кодом: низкая задержка, совместимость с картами, скинами и реплеями stable и lazer, расширяемость через плагины на Luau.

> SLAM — независимый проект и не связан с ppy Pty Ltd. «osu!» — товарный знак ppy.

## Документация
- [Архитектура](docs/ARCHITECTURE.md)
- [Дорожная карта](docs/ROADMAP.md)
- [Решения (ADR)](docs/adr/README.md)
- [Эталонная версия osu!lazer](docs/LAZER_REFERENCE.md)
- [Текущие задачи](https://github.com/sheynor43/SLAM/milestones)

## Лицензия
Код распространяется по лицензии MIT или Apache-2.0 на выбор.

Дефолтный скин в `assets/default-skin/` взят из [osu-resources](https://github.com/ppy/osu-resources) и распространяется по лицензии CC-BY-NC 4.0 — его нельзя использовать в коммерческих целях. Коммерческим форкам нужно заменить скин.

Часть логики геймплея портирована из [osu!lazer](https://github.com/ppy/osu) (MIT), см. `THIRD_PARTY_NOTICES`.

## Разработка

### Окружение (Arch Linux)

```sh
./scripts/setup-arch.sh          # установить пакеты (нужен sudo): rustup, just, cargo-deny, gh, ...
./scripts/setup-arch.sh --check  # только проверить, ничего не устанавливая
ln -sf ../../scripts/pre-commit .git/hooks/pre-commit
```

Версия Rust закреплена в `rust-toolchain.toml`, rustup поставит её сам.

### Команды

Через [`just`](https://github.com/casey/just) (полный список — `just --list`):

| Команда | Что делает |
|---|---|
| `just check` | fmt + clippy + тесты, как в CI. Запускать перед каждым коммитом |
| `just test [args]` | тесты с коротким выводом (полный лог — `target/test-output.log`) |
| `just fmt` | отформатировать код |
| `just clippy` | линтер с `-D warnings` |
| `just deny` | проверка лицензий и уязвимостей зависимостей |
| `just bench` | бенчмарки |
| `just run` | запустить клиент (release) |
| `just corpus` | прогнать корпус реплеев (`SLAM_CORPUS`, по умолчанию `../slam-corpus`) |

### Справочные данные вне репозитория

- Исходники osu!lazer для сверки правил геймплея — `../slam-refs/` (версия и команды клонирования — в [`docs/LAZER_REFERENCE.md`](docs/LAZER_REFERENCE.md)).
- Корпусы реплеев для проверки паритета — `../slam-corpus/` (или путь из `SLAM_CORPUS`).
