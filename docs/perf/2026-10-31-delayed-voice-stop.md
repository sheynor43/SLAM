# Микшер: отложенная остановка задержанного голоса

- Дата: 2026-10-31
- Задача: [#264](https://github.com/sheynor43/SLAM/issues/264), решение — [ADR-0078](../adr/0078-delayed-voice-stop.md).
- Итог: **изменений нет.** Все отличия в пределах шума, бюджет аудиоколбэка (ARCHITECTURE §18) не затронут.

## Машина
Ryzen 7 5700X, Linux 7.3.0-4-cachyos, release-профиль `cargo bench`.

## Команда
```
cargo bench -p slam-audio --bench mixer -- --save-baseline pre-0078 'mixer/process_128_frames(_controlled)?/'   # на main
cargo bench -p slam-audio --bench mixer -- --baseline pre-0078 'mixer/process_128_frames(_controlled)?/'        # на ветке
```

## Результаты (медиана criterion, буфер 128 кадров, стерео 48 кГц)

| Бенчмарк | До | После | Изменение |
|---|---|---|---|
| `process_128_frames/0` | 28,6 нс | 28,6 нс | в пределах шума |
| `process_128_frames/1` | 56,2 нс | 56,8 нс | без изменений (p = 0,28) |
| `process_128_frames/8` | 183,0 нс | 182,4 нс | без изменений |
| `process_128_frames/32` | 186,9 нс | 186,6 нс | без изменений |
| `process_128_frames/64` | 187,5 нс | 187,5 нс | в пределах шума |
| `process_128_frames_controlled/1` | 294 нс | 294 нс | без изменений |
| `process_128_frames_controlled/8` | 2,17 мкс | 2,11 мкс | −3 % (см. ниже) |
| `process_128_frames_controlled/32` | 8,39 мкс | 8,37 мкс | в пределах шума |
| `process_128_frames_controlled/64` | 16,76 мкс | 16,66 мкс | без изменений |

`controlled/8` в первом прогоне показал +4 % с широким интервалом (2,25–2,47 мкс). Два повторных прогона дали 2,11 мкс (−3 % к базе), так что это шум.
