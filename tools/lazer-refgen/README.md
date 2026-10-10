# lazer-refgen

Генератор эталонных значений для тестов паритета с osu!lazer. Считает пути слайдеров настоящим кодом лазера и osu-framework поверх настоящего osuTK и записывает точные биты результатов в `crates/slam-osu/tests/data/lazer-slider-paths.txt`. Эту фикстуру проверяет тест `crates/slam-osu/tests/slider_path_lazer.rs`.

## Запуск

Нужен .NET SDK 10 и рантайм 10 (лазер собирается под `net10.0`).

```sh
dotnet run -c Release --project tools/lazer-refgen -- crates/slam-osu/tests/data/lazer-slider-paths.txt
```

Повторный запуск даёт тот же файл: случайные случаи берутся из генератора с фиксированным сидом.

## Устройство

- `lazer/` — дословные фрагменты исходников эталонных версий из `docs/LAZER_REFERENCE.md`. Источник и строки указаны в первой строке каждого файла. Из `PathApproximator.cs` взяты только функции, нужные пути слайдера: подбор сплайнов редактора тянет лишние зависимости.
- `Support.cs` — минимальные заменители инфраструктуры лазера (`PathType`, `PathControlPoint`, `Precision`, поля и конструктор `SliderPath` без Bindable).
- `Program.cs` — набор случаев (ручные и случайные) и формат вывода.

Тригонометрия .NET (`Math.Cos`, `Math.Atan2` и т. д.) использует системную libm, поэтому фикстура привязана к платформе, где её сгенерировали (Linux x86-64).

Код лазера и osu-framework распространяется по лицензии MIT, см. `THIRD_PARTY_NOTICES`.
