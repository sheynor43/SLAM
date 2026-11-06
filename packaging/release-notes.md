> **Alpha build for testers.** Many features are missing and crashes are possible. Русская версия — ниже.

## English

### What's new in 0.1.0
The first build with all of the "playable gameplay" milestone done: maps play from start to finish with hitsounds, offsets and lazer's timing rules.

**Gameplay**
- Objects shake when a press is blocked (note lock, pressing too early).
- Toggles for slider snaking (off by default), hit lighting and cursor trail; cursor size (0.1–2.0×) with an option to scale it by circle size.
- A failed play can be saved from the fail screen (button or F1).
- Keys repeat when held: offset, speed mods, arrows in menus, settings and song select.
- Hitsound delay "as in stable" option and a stereo separation slider for positional hitsounds.
- Skin and gameplay view changes apply from the pause and fail screens without restarting the play.
- Maps without a background show the default one.
- The Windows key is blocked during play (on by default); on Linux, desktop shortcuts such as Super and Alt+Tab are blocked too.

**Sound**
- Output device, backend and buffer size change without restarting.
- The mixer runs at the device's sample rate, so 44.1 kHz devices work in Low latency and Exclusive modes.
- WASAPI Exclusive adapts to drivers that cannot keep a very short period, instead of stuttering.
- Menu music: song preview on song select, a random track in the main menu; toggle and volume in **Audio**.

**Graphics**
- Renderer setting on Windows: OpenGL by default, Direct3D 11 as an option (applies after restart). If OpenGL fails to start, SLAM retries and falls back to Direct3D 11.
- Linux video driver setting: Auto, Wayland or X11 (applies after restart).

**Menus**
- Settings open as a side panel over the current screen (Ctrl+O), with search; available on song select, pause, results and fail screens.
- Interface language: English or Русский, switched on the fly.
- Song select: background dim (70 % by default), lazer-style search (`ar>9`, `[Insane]`, `artist="xi"!`, `creator!=…`, beatmap IDs), mouse dragging of the list with inertia.

**Known limitations**
- Search keys `star`, `bpm`, `length`, `divisor`, `status` and dates are accepted but do not filter yet.
- Spinners follow lazer's rules, not stable's.
- "As in stable" hitsound delay is an estimate (~20 ms), not a measurement.
- A saved failed play cannot be watched or exported in-game yet.
- Blocking desktop shortcuts does not work through XWayland: choose Wayland in the video driver setting.

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
Open an [issue](https://github.com/sheynor43/SLAM/issues/new) with:
- the SLAM version and your OS;
- what you did, what you expected and what happened;
- the beatmap, if the problem is tied to one (a link or the file);
- the log: run SLAM from a terminal with `RUST_LOG=info` and attach the output.
  - Linux: `RUST_LOG=info ./SLAM-*.AppImage`
  - Windows (cmd): `set RUST_LOG=info` then `slam.exe`

---

## Русский

### Что нового в 0.1.0
Первая сборка с полностью закрытым этапом «играбельный геймплей»: карты играются от начала до конца, с хитсаундами, офсетами и правилами тайминга лазера.

**Игра**
- Объекты трясутся, если нажатие заблокировано (note lock, слишком раннее нажатие).
- Переключатели «змейки» слайдеров (по умолчанию выключена), подсветки попаданий и следа курсора; размер курсора (0,1–2,0×) с вариантом «по размеру кругов».
- Проваленную попытку можно сохранить с экрана провала (кнопка или F1).
- Повтор клавиш при удержании: офсет, моды скорости, стрелки в меню, настройках и выборе песни.
- Задержка хитсаундов «как в stable» и ползунок стереоразделения позиционных хитсаундов.
- Смена скина и вида игры из паузы и экрана провала — без перезапуска попытки.
- У карт без фона показывается фон по умолчанию.
- Клавиша Windows блокируется во время игры (по умолчанию включено); на Linux блокируются и сочетания рабочего стола вроде Super и Alt+Tab.

**Звук**
- Устройство вывода, бэкенд и размер буфера меняются без перезапуска.
- Микшер работает на частоте устройства, поэтому устройства на 44,1 кГц работают в режимах Low latency и Exclusive.
- WASAPI Exclusive подстраивается под драйверы, которые не держат очень короткий период, вместо заиканий.
- Музыка в меню: превью песни на выборе песни, случайный трек в главном меню; переключатель и громкость в разделе **Звук**.

**Графика**
- Настройка рендера на Windows: OpenGL по умолчанию, Direct3D 11 — по выбору (после перезапуска). Если OpenGL не запустился, SLAM повторяет попытку и переходит на Direct3D 11.
- Настройка видеодрайвера на Linux: Авто, Wayland или X11 (после перезапуска).

**Меню**
- Настройки открываются боковой панелью поверх текущего экрана (Ctrl+O), с поиском; доступны на выборе песни, в паузе, на результатах и экране провала.
- Язык интерфейса: English или Русский, переключается на лету.
- Выбор песни: затемнение фона (по умолчанию 70 %), поиск как в лазере (`ar>9`, `[Insane]`, `artist="xi"!`, `creator!=…`, номера карт), перетаскивание списка мышью с инерцией.

**Ограничения**
- Ключи поиска `star`, `bpm`, `length`, `divisor`, `status` и даты принимаются, но пока не фильтруют.
- Спиннеры работают по правилам лазера, а не stable.
- Задержка хитсаундов «как в stable» — оценка (~20 мс), а не замер.
- Сохранённую проваленную попытку пока нельзя посмотреть или экспортировать из игры.
- Блокировка сочетаний рабочего стола не работает через XWayland: выберите Wayland в настройке видеодрайвера.

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
Создайте [issue](https://github.com/sheynor43/SLAM/issues/new) и укажите:
- версию SLAM и ОС;
- что делали, что ожидали и что произошло;
- карту, если ошибка связана с ней (ссылку или файл);
- лог: запустите SLAM из терминала с `RUST_LOG=info` и приложите вывод.
  - Linux: `RUST_LOG=info ./SLAM-*.AppImage`
  - Windows (cmd): `set RUST_LOG=info`, затем `slam.exe`
