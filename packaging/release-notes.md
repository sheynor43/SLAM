> **Alpha build for testers.** Many features are missing and crashes are possible. Русская версия — ниже.

## English

### What's new in 0.1.1
A refreshed interface and the first part of the "Skins" milestone: the gameplay HUD now comes from your skin.

**Skins and HUD**
- Score, accuracy and combo are drawn with the skin's digits and roll smoothly; the health bar comes from the skin too. HUD animations can be turned off.
- Built-in HUD layouts with a song progress bar, a key overlay and a skinned skip button. Choosing a layout in the settings comes later.
- An image in a skin that can't be decoded counts as missing (the default one is used) instead of breaking the skin.

**Interface**
- New look: rounded, slanted buttons with layered gradients, outlined triangles and shadows; colour schemes; separate interface and gameplay scale.
- Song select at lazer's proportions: beatmap thumbnails, a statistics panel and local scores on the left, star ratings (calculated in the background and cached), and difficulties sorted by stars that slide open under their set.
- Search also understands text typed in the other keyboard layout (Russian ↔ English).
- Menu background, menu sounds, parallax, blur, glow and shadows, each with its own strength (0–200 %) and an off switch.
- Screen transitions: none, simple, band, slices, iris, shards, shutter or random.
- Menu and gameplay cursors: SLAM's own arrow (spin at rest, trail, press rings, all optional), the skin's, or the system cursor.
- Settings are one continuous list that remembers where you were, with a new **Skin** section.
- Pause menu styles, including one like stable's.
- A startup intro with its own sound (can be turned off).
- FPS counter modes, from a single figure up to a panel with the draw, update, audio and input threads and frame-time graphs.
- **Feedback** section in the settings: opens a pre-filled bug report or suggestion form in the browser.

**Known limitations**
- Hot-reloading skins, skin profiles, `.osk` import and choosing HUD layouts in the settings are not done yet.
- The skin's score and accuracy sit 10–13 px further right than in lazer and stable at 1920×1080.
- Search keys `star`, `bpm`, `length`, `divisor`, `status` and dates are accepted but do not filter yet.
- Spinners follow lazer's rules, not stable's.
- The new interface effects have not been checked on Direct3D 11 yet; if something looks wrong, try OpenGL.

### Download
- **Windows 10+ x64:** `SLAM-…-windows-x64.zip`
- **Linux x64:** `SLAM-…-x86_64.AppImage`

### Requirements
- A graphics card with OpenGL 3.3 (on Windows, Direct3D 11 also works).
- Linux: X11 or Wayland, PipeWire for sound, glibc as new as Ubuntu 24.04 or newer. For the folder picker: xdg-desktop-portal or zenity.

### Running
- **Windows:** extract the zip anywhere and run `slam.exe`.
- **Linux:** `chmod +x SLAM-*.AppImage`, then run the file.
- If your beatmap folder was not found automatically, choose it in the settings: osu!stable's `Songs` or any folder with extracted `.osz` files.

### Troubleshooting
- **Windows says "Windows protected your PC".** The build is not signed. Click **More info → Run anyway**.
- **The AppImage does not start and mentions FUSE.** Install `fuse3` (or `fuse2`) with your package manager, or run it with `--appimage-extract-and-run`.
- **The AppImage says a `GLIBC_…` version is not found.** Your distribution is older than Ubuntu 24.04; this alpha does not support it.
- **No sound on Linux.** SLAM plays through PipeWire: check that it is running (`systemctl --user status pipewire`).
- **Crackling sound.** Raise `buffer_frames` in the `[audio]` section of `slam.toml` in the settings folder (see below), e.g. to the next power of two.
- **Black screen or an OpenGL error.** Update the graphics driver; on laptops, run SLAM on the discrete GPU.
- **Stutter while recording the screen (OBS) on Linux.** SLAM asks the GPU for high priority, but the kernel grants it only with the `CAP_SYS_NICE` capability, which an AppImage cannot carry. Extract it with `./SLAM-*.AppImage --appimage-extract`, run `sudo setcap cap_sys_nice+ep squashfs-root/usr/bin/slam` and start `squashfs-root/AppRun`. Repeat after each update.
- **Black or broken screen after changing the screen mode or display.** In the `[window]` section of `slam.toml` (see below) set `mode = "windowed"` and `display = 0`, or delete the `[window]` section.
- **Start from clean settings.** Delete (or move away) the settings folder:
  - Windows: `%APPDATA%\SLAM`
  - Linux: `~/.config/slam` (settings) and `~/.local/share/slam` (replays, cache)
- **Portable mode.** Put an empty file named `portable` next to `slam.exe`: settings and data are then kept next to it.

### Reporting a bug
The quickest way is **Settings → Feedback**: it opens a form with the version and your system filled in. Or open an [issue](https://github.com/sheynor43/SLAM/issues/new) with:
- the SLAM version and your OS;
- what you did, what you expected and what happened;
- the beatmap, if the problem is tied to one (a link or the file);
- the log: run SLAM from a terminal with `RUST_LOG=info` and attach the output.
  - Linux: `RUST_LOG=info ./SLAM-*.AppImage`
  - Windows (cmd): `set RUST_LOG=info` then `slam.exe`

---

## Русский

### Что нового в 0.1.1
Обновлённый интерфейс и первая часть этапа «Скины»: игровой HUD теперь берётся из скина.

**Скины и HUD**
- Счёт, точность и комбо рисуются цифрами скина и плавно прокручиваются; полоска здоровья — тоже из скина. Анимации HUD можно выключить.
- Встроенные раскладки HUD с прогрессом песни, оверлеем клавиш и кнопкой пропуска из скина. Выбор раскладки в настройках появится позже.
- Изображение скина, которое не удалось декодировать, считается отсутствующим (берётся стандартное), а не ломает скин.

**Интерфейс**
- Новый вид: скруглённые наклонные кнопки со слоями градиента, контурными треугольниками и тенями; цветовые схемы; отдельный масштаб меню и игры.
- Выбор песни в пропорциях лазера: миниатюры карт, панель характеристик и локальные рекорды слева, звёздный рейтинг (считается в фоне и кешируется), сложности по звёздам, плавно раскрываются под своим сетом.
- Поиск понимает текст, набранный в другой раскладке (русская ↔ английская).
- Фон и звуки меню, параллакс, размытие, свечение и тени — у каждого своя сила (0–200 %) и выключатель.
- Переходы между экранами: нет, простой, полоса, полосы, круг, осколки, створки или случайный.
- Курсор меню и игры: свой курсор-стрелка SLAM (вращение в покое, шлейф, кольца нажатий — всё отключаемо), из скина или системный.
- Настройки — один сплошной список, который помнит место; новый раздел **Скин**.
- Стили меню паузы, в том числе как в stable.
- Заставка при запуске со своим звуком (можно выключить).
- Режимы счётчика FPS: от одного числа до панели с потоками отрисовки, update, звука и ввода и графиками времени кадра.
- Раздел **Связь** в настройках: открывает в браузере заполненную форму сообщения об ошибке или предложения.

**Известные ограничения**
- Горячей перезагрузки скинов, профилей скина, импорта `.osk` и выбора раскладки HUD в настройках пока нет.
- Счёт и точность скина при 1920×1080 стоят на 10–13 px правее, чем в лазере и stable.
- Ключи поиска `star`, `bpm`, `length`, `divisor`, `status` и даты принимаются, но пока не фильтруют.
- Спиннеры работают по правилам лазера, а не stable.
- Новые эффекты интерфейса ещё не проверены на Direct3D 11; если что-то выглядит не так, попробуйте OpenGL.

### Скачать
- **Windows 10+ x64:** `SLAM-…-windows-x64.zip`
- **Linux x64:** `SLAM-…-x86_64.AppImage`

### Требования
- Видеокарта с OpenGL 3.3 (на Windows подойдёт и Direct3D 11).
- Linux: X11 или Wayland, PipeWire для звука, glibc не старше Ubuntu 24.04. Для выбора папки — xdg-desktop-portal или zenity.

### Запуск
- **Windows:** распакуйте архив в любую папку и запустите `slam.exe`.
- **Linux:** `chmod +x SLAM-*.AppImage`, затем запустите файл.
- Если папка с картами не нашлась сама, укажите её в настройках: `Songs` из osu!stable или любая папка с распакованными `.osz`.

### Если что-то не так
- **Windows пишет «Система Windows защитила ваш компьютер».** Сборка не подписана. Нажмите **Подробнее → Выполнить в любом случае**.
- **AppImage не запускается и упоминает FUSE.** Установите `fuse3` (или `fuse2`) пакетным менеджером или запустите с ключом `--appimage-extract-and-run`.
- **AppImage пишет, что не найдена версия `GLIBC_…`.** Дистрибутив старше Ubuntu 24.04; эта альфа его не поддерживает.
- **Нет звука на Linux.** SLAM играет через PipeWire: проверьте, что он запущен (`systemctl --user status pipewire`).
- **Треск в звуке.** Увеличьте `buffer_frames` в разделе `[audio]` файла `slam.toml` в папке настроек (см. ниже), например до следующей степени двойки.
- **Чёрный экран или ошибка OpenGL.** Обновите драйвер видеокарты; на ноутбуке запускайте SLAM на дискретной видеокарте.
- **Подтормаживания при записи экрана (OBS) на Linux.** SLAM просит у видеокарты высокий приоритет, но ядро даёт его только с правом `CAP_SYS_NICE`, которое AppImage не может нести. Распакуйте его: `./SLAM-*.AppImage --appimage-extract`, выполните `sudo setcap cap_sys_nice+ep squashfs-root/usr/bin/slam` и запускайте `squashfs-root/AppRun`. После каждого обновления — заново.
- **Чёрный или сломанный экран после смены режима экрана или монитора.** В разделе `[window]` файла `slam.toml` (см. ниже) задайте `mode = "windowed"` и `display = 0` или удалите раздел `[window]`.
- **Начать с чистых настроек.** Удалите (или перенесите) папку настроек:
  - Windows: `%APPDATA%\SLAM`
  - Linux: `~/.config/slam` (настройки) и `~/.local/share/slam` (реплеи, кэш)
- **Портативный режим.** Положите пустой файл `portable` рядом с `slam.exe` — настройки и данные будут храниться рядом с ним.

### Как сообщить об ошибке
Быстрее всего — **Настройки → Связь**: откроется форма, где уже указаны версия и система. Или создайте [issue](https://github.com/sheynor43/SLAM/issues/new) и укажите:
- версию SLAM и ОС;
- что делали, что ожидали и что произошло;
- карту, если ошибка связана с ней (ссылку или файл);
- лог: запустите SLAM из терминала с `RUST_LOG=info` и приложите вывод.
  - Linux: `RUST_LOG=info ./SLAM-*.AppImage`
  - Windows (cmd): `set RUST_LOG=info`, затем `slam.exe`

