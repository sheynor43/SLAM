> **Alpha build for testers.** Many features are missing and crashes are possible. Русская версия — ниже.

## English

### What's new in 0.0.2
- **Screen modes:** windowed, borderless and fullscreen, with display and resolution choice (**Settings → Layout**). F11 or Alt+Enter cycles the mode. Fullscreen gives the lowest input latency.
- The windowed size and position are remembered between launches.
- **Minimise on focus loss:** in fullscreen, SLAM minimises when you switch to another app (can be turned off in **Layout**).
- **High precision mouse** (raw relative input) and **cursor sensitivity**, as in lazer. High precision is on by default on Windows only.
- Unstable rate, average hit error and median after each play are written to the log (run with `RUST_LOG=info`, see below).

### Download
- **Windows 10+ x64:** `SLAM-…-windows-x64.zip`
- **Linux x64:** `SLAM-…-x86_64.AppImage`

### Requirements
- A graphics card with OpenGL 3.3.
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

### Что нового в 0.0.2
- **Режимы экрана:** в окне, без рамки и полноэкранный, с выбором монитора и разрешения (**Настройки → Экран**). F11 или Alt+Enter переключают режим. Минимальная задержка ввода — в полноэкранном режиме.
- Размер и положение окна запоминаются между запусками.
- **Сворачивание при потере фокуса:** в полноэкранном режиме SLAM сворачивается при переключении на другое приложение (отключается в разделе **Экран**).
- **Высокоточная мышь** (сырой относительный ввод) и **чувствительность курсора**, как в лазере. Высокоточная мышь по умолчанию включена только на Windows.
- Нестабильность (UR), средняя ошибка попадания и медиана после каждой игры пишутся в лог (запуск с `RUST_LOG=info`, см. ниже).

### Скачать
- **Windows 10+ x64:** `SLAM-…-windows-x64.zip`
- **Linux x64:** `SLAM-…-x86_64.AppImage`

### Требования
- Видеокарта с OpenGL 3.3.
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
