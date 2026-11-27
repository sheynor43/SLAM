> **Alpha build for testers.** Many features are missing and crashes are possible. Русская версия — ниже.

## English

### What's new in 0.1.5
**Before-play screen and transitions**
- A new screen before playing shows the beatmap card and lets you adjust quick settings for visuals, sound and input.
- Starting a map waits until preparation is complete and keeps a short minimum preview time, so the transition does not reveal an unready game.
- The fade-to-black transition, player highlight and difficulty-colour outline are smoother and configurable through **Settings → General → Transition length**.

**Self-update**
- Signed updates are downloaded from GitHub Releases and verified with the project's Ed25519 key before installation.

**Interface polish**
- The player panel now opens and closes cleanly, including its cover animation and the transition from the previous track.
- Quick-setting popups keep their arrows attached while they are open.

**Known limitations**
- Combo bursts are not drawn yet.
- Results opened from a local score have no hit error lines or health graph.
- Hot-reloading skins, skin profiles, `.osk` import and choosing HUD layouts in the settings are not done yet.
- Spinners, scoring and health follow lazer's rules, not stable's.
- The new interface has not been checked on Direct3D 11 yet; if something looks wrong, try OpenGL.

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

### Что нового в 0.1.5
**Экран перед игрой и переходы**
- Новый экран перед игрой показывает карточку карты и позволяет быстро настроить визуал, звук и ввод.
- Запуск карты ждёт завершения подготовки и минимального времени предпросмотра, поэтому переход не показывает неготовую игру.
- Переход в чёрный, подсветка плеера и обводка цветом сложности стали плавнее; длина перехода настраивается в **Настройки → Общее → Длина перехода**.

**Обновление**
- Подписанные обновления загружаются из GitHub Releases и проверяются ключом проекта Ed25519 перед установкой.

**Доработки интерфейса**
- Панель плеера теперь аккуратно открывается и закрывается, включая анимацию обложки и смену предыдущего трека.
- Стрелки всплывающих окон быстрых настроек остаются прикреплёнными во время открытия.

**Известные ограничения**
- Комбо-взрывы пока не рисуются.
- У результатов, открытых из локального рекорда, нет строк ошибки попаданий и графика здоровья.
- Горячей перезагрузки скинов, профилей скина, импорта `.osk` и выбора раскладки HUD в настройках пока нет.
- Спиннеры, очки и здоровье работают по правилам лазера, а не stable.
- Новый интерфейс ещё не проверен на Direct3D 11; если что-то выглядит не так, попробуйте OpenGL.

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

