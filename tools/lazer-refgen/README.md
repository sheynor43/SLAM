# lazer-refgen

Генератор эталонных значений для тестов паритета с osu!lazer. Считает настоящим кодом лазера и osu-framework поверх настоящего osuTK и рантайма .NET и записывает точные биты результатов в три фикстуры:

| Фикстура | Что в ней | Тест |
|---|---|---|
| `crates/slam-osu/tests/data/lazer-slider-paths.txt` | пути слайдеров, операции osuTK | `crates/slam-osu/tests/slider_path_lazer.rs` |
| `crates/slam-osu/tests/data/lazer-slider-nested.txt` | параметры сложности слайдера (скорость, расстояние тиков, preempt, fade-in, масштаб), вложенные объекты, события `SliderEventGenerator`, результаты `List<T>.Sort` | `crates/slam-osu/tests/slider_nested_lazer.rs`, юнит-тест `dotnet::tests::list_sort_matches_dotnet` |
| `crates/slam-osu/tests/data/lazer-stacking.txt` | стэкинг (`OsuBeatmapProcessor.ApplyStacking`, оба алгоритма): маленькие карты в виде строк `.osu`, высоты стэков и смещённые позиции объектов | `crates/slam-osu/tests/stacking_lazer.rs` |

Длинные списки вложенных объектов и событий записываются сокращённо: первые и последние строки плюс FNV-1a-хэш всех строк.

## Запуск

Нужен .NET SDK 10 и рантайм 10 (лазер собирается под `net10.0`).

```sh
dotnet run -c Release --project tools/lazer-refgen -- crates/slam-osu/tests/data/lazer-slider-paths.txt crates/slam-osu/tests/data/lazer-slider-nested.txt crates/slam-osu/tests/data/lazer-stacking.txt
```

Повторный запуск даёт тот же файл: случайные случаи берутся из генератора с фиксированным сидом.

## Устройство

- `lazer/` — дословные фрагменты исходников эталонных версий из `docs/LAZER_REFERENCE.md`. Источник и строки указаны в первой строке каждого файла. Из `PathApproximator.cs` взяты только функции, нужные пути слайдера: подбор сплайнов редактора тянет лишние зависимости.
- `lazer/OsuObjects.cs` — дословные тела методов `ApplyDefaultsToSelf`, `CreateNestedHitObjects` и `HitObject.ApplyDefaults` в минимальных оболочках классов (без Bindable, сэмплов, судейства и хит-окон); диапазоны строк указаны в шапке файла. Вызов `UpdateNestedSamples` опущен: сэмплы проверяются тестами Rust.
- `lazer/OsuBeatmapProcessor.cs` — дословные `ApplyStacking`, `applyStacking`, `applyStackingOld` и `calculateStackThreshold`; класс сделан статическим, `PreProcess`/`PostProcess` опущены (им нужен `BeatmapProcessor` лазера). `lazer/Vector2Extensions.cs` — дословный `Distance` из osu-framework. В `lazer/OsuObjects.cs` добавлены дословные `StackOffset`, `StackedPosition`, `StackedEndPosition`, `Slider.Duration` и оболочка `Spinner` с её `EndTime`/`Duration`.
- `Support.cs` — минимальные заменители инфраструктуры лазера (`PathType`, `PathControlPoint`, `Precision`, поля и конструктор `SliderPath` без Bindable, интерфейсы `IHasPath`/`IHasRepeats`/`IHasSliderVelocity`, дословные `IHasDuration` и `GetEndTime`, `IBeatmap` только с полями, нужными стэкингу, `ControlPointInfo` с одной точкой тайминга).
- `Program.cs` — случаи путей (ручные и случайные) и формат вывода; `Nested.cs` — случаи слайдеров, событий генератора и сортировки; `Stacking.cs` — случаи стэкинга. Каждый случай стэкинга — текст карты `.osu`; генератор разбирает свои же строки так, как их разбирает декодер лазера (координаты усекаются до `int` до версии формата 128), а тест Rust декодирует тот же текст. Смещение 24 мс для версий до 5 не применяется ни там, ни там.

Тригонометрия .NET (`Math.Cos`, `Math.Atan2` и т. д.) использует системную libm, поэтому фикстура привязана к платформе, где её сгенерировали (Linux x86-64).

Код лазера и osu-framework распространяется по лицензии MIT, см. `THIRD_PARTY_NOTICES`.
