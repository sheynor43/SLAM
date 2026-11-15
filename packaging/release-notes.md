> **Alpha build for testers.** Many features are missing and crashes are possible. Русская версия — ниже.

## English

### What's new in 0.1.2
**Fixed: SLAM did not start on Windows.** 0.1.1 showed a black window for a second and closed: the game ran out of the main thread's stack while starting, which on Windows is eight times smaller than on Linux. Fixed.

**When something goes wrong**
- SLAM now keeps a log of the last five runs in the `logs` folder (see "Reporting a bug" below), so there is no need to run it from a terminal to report a problem.
- If SLAM stops with an error, a window shows the error and where the log is.

**Skins**
- The skin's countdown before the first note (`ready`, `count3`…`go` with their sounds), warning arrows before a break ends and `section-pass`/`section-fail` in breaks.
- The skip button can be clicked with the mouse or pressed with the gameplay keys while the cursor is over it.
- Gameplay settings to turn the arrows, the countdown and its sounds off, and to choose what breaks show: the skin's pass/fail, the time left as in lazer, SLAM's own countdown or nothing.

**Known limitations**
- Hot-reloading skins, skin profiles, `.osk` import and choosing HUD layouts in the settings are not done yet.
- The skin's score and accuracy sit 10–13 px further right than in lazer and stable at 1920×1080.
- The countdown's timing follows the osu! wiki and has not been compared with stable yet.
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
- the log `slam.log` from the `logs` folder (if SLAM stops with an error, the error window shows where it is):
  - Windows: `%APPDATA%\SLAM\logs` (portable mode: `logs` next to `slam.exe`)
  - Linux: `~/.local/share/slam/logs`

---

## Русский

### Что нового в 0.1.2
**Исправлено: SLAM не запускался на Windows.** 0.1.1 на секунду показывал чёрное окно и закрывался: при запуске игре не хватало стека главного потока, а на Windows он в восемь раз меньше, чем на Linux. Исправлено.

**Если что-то пошло не так**
- SLAM ведёт лог пяти последних запусков в папке `logs` (см. «Как сообщить об ошибке» ниже): запускать игру из терминала, чтобы сообщить о проблеме, больше не нужно.
- Если SLAM остановился с ошибкой, окно показывает ошибку и путь к логу.

**Скины**
- Отсчёт скина перед первой нотой (`ready`, `count3`…`go` со звуками), стрелки-предупреждения перед концом перерыва и `section-pass`/`section-fail` в перерывах.
- Кнопку пропуска можно нажать мышью или игровыми клавишами, когда курсор над ней.
- В настройках игры: выключатели стрелок, отсчёта и его звуков и выбор, что показывать в перерывах: pass/fail скина, оставшееся время как в лазере, отсчёт SLAM или ничего.

**Известные ограничения**
- Горячей перезагрузки скинов, профилей скина, импорта `.osk` и выбора раскладки HUD в настройках пока нет.
- Счёт и точность скина при 1920×1080 стоят на 10–13 px правее, чем в лазере и stable.
- Тайминг отсчёта взят из wiki osu! и ещё не сверен со stable.
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
- лог `slam.log` из папки `logs` (если SLAM остановился с ошибкой, окно ошибки показывает, где он):
  - Windows: `%APPDATA%\SLAM\logs` (портативный режим: `logs` рядом с `slam.exe`)
  - Linux: `~/.local/share/slam/logs`

