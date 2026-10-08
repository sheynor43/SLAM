# Эталонная версия osu!lazer

SLAM сверяет правила геймплея, формулы и обработку форматов с конкретной версией osu!lazer. Исходники лежат в `../slam-refs/` (вне репозитория, только чтение, клон без истории).

| Репозиторий | Тег | Коммит | Путь |
|---|---|---|---|
| [ppy/osu](https://github.com/ppy/osu) | `2026.1005.0-lazer` | `9a10d935b05a3b896f4bd811d620f2023331db93` | `../slam-refs/osu` |
| [ppy/osu-framework](https://github.com/ppy/osu-framework) | `2026.921.1` | `37c3e328c8cc7a018234ec78982ea12d3712096a` | `../slam-refs/osu-framework` |

- Дата фиксации эталона: 2026-10-08.
- Версия `osu-framework` взята из `osu.Game/osu.Game.csproj` (`ppy.osu.Framework` `2026.921.1`), тег совпадает точно.
- Метка в портированном коде: `// Ported from osu!lazer 2026.1005.0-lazer: <path>`.

## Восстановление

```sh
git clone --depth 1 --branch 2026.1005.0-lazer https://github.com/ppy/osu ../slam-refs/osu
git clone --depth 1 --branch 2026.921.1 https://github.com/ppy/osu-framework ../slam-refs/osu-framework
```

## Правило обновления

Обновление эталона — отдельная задача (своя ветка и сессия):

1. Клонировать новые теги, обновить таблицу выше.
2. Просмотреть изменения в портированных файлах (по меткам `Ported from osu!lazer`) и перенести их.
3. Прогнать корпус реплеев (`just corpus`), разобрать все расхождения.
4. Только после зелёного корпуса слить изменения.
