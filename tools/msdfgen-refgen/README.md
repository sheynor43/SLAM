# msdfgen-refgen

Генератор эталонной фикстуры `crates/slam-text/tests/data/msdfgen-corrected.txt` для теста `corrected_fields_match_msdfgen` в `crates/slam-text/src/msdf/tests.rs`. Поля считает настоящий msdfgen v1.13 ([ADR-0062](../../docs/adr/0062-own-msdf-generator.md)). Конвейер такой: `normalize`, `edgeColoringSimple` (3 рад, зерно 0), `generateMSDF` с перекрытиями и без коррекции, `distanceSignCorrection` (`FILL_NONZERO`), `msdfErrorCorrection` (`EDGE_PRIORITY`, `DO_NOT_CHECK_DISTANCE`).

Фигуры продублированы из `reference_shape` в тестах. Если меняешь их там, поменяй и здесь. Суффикс `@scaled` у имени задаёт неравномерную проекцию: масштаб (1.5, 2), сдвиг (0.5, −0.25), диапазон 3.

Генератора для `msdfgen-fields.txt` (поля без коррекции, цвета рёбер, точки после `normalize`) в репозитории нет. Поля без коррекции можно получить, если убрать вызов `msdfErrorCorrection`.

## Запуск

Нужны `g++` и клон msdfgen с тегом `v1.13`. Арифметика должна совпадать с Rust побитово, поэтому FMA выключен (`-ffp-contract=off`).

```sh
M=../slam-refs/msdfgen
g++ -O2 -ffp-contract=off -std=c++11 -DMSDFGEN_PUBLIC= -I$M -I$M/core \
    tools/msdfgen-refgen/refgen.cpp $M/core/*.cpp -o target/msdfgen-refgen
OUT=crates/slam-text/tests/data/msdfgen-corrected.txt
{
  head -4 $OUT
  { grep '^field' crates/slam-text/tests/data/msdfgen-fields.txt | awk '{printf "%s %s %s ", $2, $3, $4}'
    echo "self_overlap_star@scaled 22 28 cusp_both_degenerate@scaled 22 28 overlap@scaled 19 22"
  } | xargs target/msdfgen-refgen
} > $OUT.new && mv $OUT.new $OUT
```

В stderr выводится число исправленных текселей по каждой фигуре. Тест проверяет сумму (сейчас 195).
