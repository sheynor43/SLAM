# Эталонная версия osu!lazer

SLAM сверяет правила геймплея, формулы и обработку форматов с конкретной версией osu!lazer. Исходники лежат в `../slam-refs/` (вне репозитория, только чтение, клон без истории).

| Репозиторий | Тег | Коммит | Путь |
|---|---|---|---|
| [ppy/osu](https://github.com/ppy/osu) | `2026.1005.0-lazer` | `9a10d935b05a3b896f4bd811d620f2023331db93` | `../slam-refs/osu` |
| [ppy/osu-framework](https://github.com/ppy/osu-framework) | `2026.921.1` | `37c3e328c8cc7a018234ec78982ea12d3712096a` | `../slam-refs/osu-framework` |
| [ppy/osu-resources](https://github.com/ppy/osu-resources) | `2026.918.0` | `d481bc98f2603348ee81c15174cd72604286310d` | `../slam-refs/osu-resources` |

- Дата фиксации эталона: 2026-10-08.
- Версия `osu-framework` взята из `osu.Game/osu.Game.csproj` (`ppy.osu.Framework` `2026.921.1`), тег совпадает точно.
- Версия `osu-resources` взята оттуда же (`ppy.osu.Game.Resources` `2026.918.0`). Это источник дефолтного скина (`osu.Game.Resources/Skins/Legacy`), лицензия CC-BY-NC 4.0 (`LICENCE.md`).
- Метка в портированном коде: `// Ported from osu!lazer 2026.1005.0-lazer: <path>`.

## Восстановление

```sh
git clone --depth 1 --branch 2026.1005.0-lazer https://github.com/ppy/osu ../slam-refs/osu
git clone --depth 1 --branch 2026.921.1 https://github.com/ppy/osu-framework ../slam-refs/osu-framework
git clone --depth 1 --branch 2026.918.0 https://github.com/ppy/osu-resources ../slam-refs/osu-resources
```

## Правило обновления

Обновление эталона — отдельная задача (своя ветка и сессия):

1. Клонировать новые теги, обновить таблицу выше.
2. Просмотреть изменения в портированных файлах (по меткам `Ported from osu!lazer`) и перенести их.
3. Прогнать корпус реплеев (`just corpus`), разобрать все расхождения.
4. Только после зелёного корпуса слить изменения.

## Прочие эталоны

| Репозиторий | Тег | Коммит | Путь | Для чего |
|---|---|---|---|---|
| [Chlumsky/msdfgen](https://github.com/Chlumsky/msdfgen) (MIT) | `v1.13` | `1874bcf7d9624ccc85b4bc9a85d78116f690f35b` | `../slam-refs/msdfgen` | генератор MSDF в `slam-text` ([ADR-0062](adr/0062-own-msdf-generator.md)) |

- Метка в портированном коде: `// Ported from msdfgen v1.13: <path>`.
- Восстановление: `git clone --depth 1 --branch v1.13 https://github.com/Chlumsky/msdfgen ../slam-refs/msdfgen`.
