# Корпус сторонних скинов

Локальный набор скинов stable от сторонних авторов для скриншот-тестов. Закрывает критерий M3 «скины из корпуса рисуются без артефактов» ([ADR-0111](adr/0111-skin-screenshot-tests.md)): каждый скин рисует сцену `scene` в те же моменты, что и скин по умолчанию (900, 1700, 3900, 6450 мс), и кадры сравниваются с эталонами с тем же допуском.

Скины и эталоны **не коммитятся** ни в какой репозиторий: у скинов свои лицензии и размер. Корпус лежит рядом с репозиторием, как корпус реплеев (`../slam-corpus`). Путь задаётся `SLAM_SKIN_CORPUS` (по умолчанию `../slam-skins` относительно репозитория SLAM). Эталоны, как и в тесте скина по умолчанию, зависят от Mesa llvmpipe.

## Структура

```
slam-skins/
  manifest.toml
  skins/<папка>/            # распакованный скин, внутри skin.ini
  reference/<имя>-<время>.png
```

## manifest.toml

По одной записи `[[skin]]` на скин. Неизвестные ключи — ошибка.

```toml
[[skin]]
name = "example"                  # уникальное, [a-z0-9-]; префикс имён эталонов
folder = "Example Skin"           # папка в skins/
version = "2.5"                   # ожидаемая Version из skin.ini ("1.0", "2.5", "latest"); необязательно
url = "https://example.com/skin"  # откуда скачан
hd = true                         # есть ли @2x
spinner = "new"                   # "old" | "new"
notes = "анимированный курсор"    # необязательно
known_issues = ["#123"]           # необязательно: известные проблемы, только для справки
```

Проверки: повторяющиеся имена, отсутствующая папка и несовпадение `version` с версией из разобранного `skin.ini` дают ошибку этого скина; остальные скины всё равно прогоняются, в конце тест падает со списком.

## Команды

- `just skin-corpus [ФИЛЬТР]` — сравнить кадры с эталонами; `ФИЛЬТР` — подстрока имени скина.
- `just skin-corpus-update [ФИЛЬТР]` — записать эталоны в `slam-skins/reference`; каждый изменённый эталон просмотреть.

При расхождении кадры и карты разницы лежат в `target/tmp/skin-corpus` (`<имя>-<время>.actual.png`, `.diff.png`). Тест игнорируется по умолчанию (нужны Mesa и корпус) и в CI не запускается.

## Список скинов

| Имя | Версия | @2x | Спиннер | Ссылка | Заметки |
|---|---|---|---|---|---|
| retro | нет `skin.ini` | нет | старый | [ссылка](https://github.com/ppy/osu-resources/tree/master/osu.Game.Resources/Skins/Retro) | osu!classic: без `skin.ini` (путь 1.0), `sliderb0`–`9` |
| legacy | нет `skin.ini` | да | новый | [ссылка](https://github.com/ppy/osu-resources/tree/master/osu.Game.Resources/Skins/Legacy) | дефолт stable: без `skin.ini`, только `@2x` |
| whitecat-nm | 2.5 | да | старый | [ссылка](https://github.com/myangelaku/whitecat-skins/tree/master) | WhiteCat (1.0) NM: префиксы в подпапках, 270+ кадров анимации, `AnimationFramerate: 60`, окраска шара, `SliderBallFlip` |
| whitecat-2-old | 2.5 | да | старый | [ссылка](https://github.com/myangelaku/whitecat-skins/tree/master) | CK WhiteCat 2.0 old: один префикс у счёта и комбо |
| rafis-prawilosc | 2.2 | да | старый | [ссылка](https://github.com/myangelaku/whitecat-skins/tree/master/Rafis%202018-03-26%20HDDT) | Rafis 2018 HDDT (Prawilosc vJP): правила версии 2.2 |
| vaxei-bacon | 2.5 | да | старый | [ссылка](https://github.com/ryancranie/skinhub/tree/tyfh/universal) | CK Bacon boi 1.0 (Vaxei): комментарий `//` после цвета |
| marisancino | 2.5 | да | старый | [ссылка](https://github.com/ryancranie/skinhub/tree/tyfh/player/arancino) | в стиле Cookiezi, префиксы `score-` и `combo-` |
| yugen | 2.4 | да | старый | [ссылка](https://github.com/ryancranie/skinhub/tree/tyfh/universal) | YUGEN FINAL Widescreen: анимированные `hit300`/`300g`/`300k`, `SpinnerFadePlayfield` |
| pendual | 2.4 | да | старый | [ссылка](https://github.com/myangelaku/whitecat-skins/tree/master) | PENDUAL: анимированный `sliderb`, комбо шрифтом счёта, `Skin.ini` с заглавной |
| skinnotfound-v2 | 3.14 (неверная) | да | старый | [ссылка](https://github.com/myangelaku/whitecat-skins/tree/master) | `HitCircleOverlayAboveNumber: 0`, `AnimationFramerate: -1` |
| btmc-freedom-dive | 2.7 (вне stable) | да | старый | [ссылка](https://github.com/ryancranie/skinhub/tree/tyfh/universal) | оверлей под номером |
| sameskin-noring | latest | нет | новый | [ссылка](https://github.com/myangelaku/whitecat-skins/tree/master/SameSkinButNoRingHitCirclesWeirdName) | SD с новым спиннером, необычные имена файлов курсора |
| millhiore-lite | latest | нет | новый | [ссылка](https://github.com/myangelaku/whitecat-skins/tree/master/Millhiore%20Lite) | 54 файла, почти всё из фолбэков; новый спиннер и `spinner-circle` |
| vv-idke-trail | 2.4 | да | старый | [ссылка](https://github.com/ryancranie/skinhub/tree/tyfh/universal) | обратная косая черта в префиксе шрифта (`num\berlin`) |
| thinkingofu | latest | нет | старый | [ссылка](https://github.com/ryancranie/skinhub/tree/tyfh/player/asterllia) | только SD, `ComboPrefix: hide` |

Спиннер — по правилу лазера: новый, если есть `spinner-top` и нет `spinner-background`. Retro и Legacy — из `ppy/osu-resources` (CC-BY-NC 4.0); у остальных архивов лицензии нет, поэтому скины только локальные.

Чего в корпусе нет: скина с явной `Version: 1.0`, анимированного `hitcircle`/`hitcircleoverlay`, HD-скина с новым спиннером и `skin.ini`.
