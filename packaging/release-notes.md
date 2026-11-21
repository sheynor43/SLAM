> **Alpha build for testers.** Many features are missing and crashes are possible. Русская версия — ниже.

## English

### What's new in 0.1.4
**Skins and effects**
- Results screen in stable's look when the skin has `ranking-panel`: its header, the beatmap's background, the skin's Back and Retry, the performance graph with a health line and a details page below (scroll down). Switch it off in **Settings → Skin → Results screen style**.
- Kiai flashes on hit circles and on slider heads, tails and repeats.
- Cursor effects: `star2` particles in kiai and breaks, cursor smoke (hold the smoke key) and ripples on presses (**Settings → Skin → Cursor ripples**).
- Pause and fail menus play the skin's sounds and `pause-loop`.

**Gameplay**
- The cursor is drawn at the mouse's latest position every frame: smoother, with less delay. Hits are judged as before.
- Windows: in borderless mode the sensitivity no longer resets after the cursor reaches the screen edge.
- Skipping the intro dips to black and fades the music; the skip button has three styles (skin, lazer, SLAM) in **Settings → Skin**.
- Restarting darkens the screen while quick retry is held, then fades the new play in with a sound; switch it off in **Settings → Gameplay → Restart animation and sound**.
- The hold delay of quick retry and quick exit can be set (**Settings → Gameplay**).

**Interface**
- Local scores in song select open their results screen.
- Settings: sections are icon buttons in a narrower panel.

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

### Что нового в 0.1.4
**Скины и эффекты**
- Экран результатов в виде stable, если в скине есть `ranking-panel`: шапка, фон карты, кнопки «Назад» и «Заново» из скина, график с линией здоровья и страница подробностей ниже (прокрутите вниз). Выключается в **Настройки → Скин → Вид экрана результатов**.
- Вспышки kiai у кругов, головок, хвостов и повторов слайдеров.
- Эффекты курсора: частицы `star2` в kiai и перерывах, дым курсора (удерживайте клавишу дыма) и рябь от нажатий (**Настройки → Скин → Рябь от нажатий**).
- Меню паузы и провала играют звуки скина и `pause-loop`.

**Игра**
- Курсор рисуется по самой свежей позиции мыши в каждом кадре: плавнее и с меньшей задержкой. Попадания считаются как раньше.
- Windows: в безрамочном режиме чувствительность больше не сбрасывается, когда курсор доходит до края экрана.
- Пропуск заставки уводит картинку в чёрное и приглушает музыку; у кнопки пропуска три вида (скин, лазер, SLAM) в **Настройки → Скин**.
- Рестарт: пока удерживается быстрый рестарт, экран темнеет, затем новая игра проявляется со звуком; выключается в **Настройки → Игра → Анимация и звук рестарта**.
- Задержку удержания для быстрого рестарта и выхода можно настроить (**Настройки → Игра**).

**Интерфейс**
- Локальные рекорды в выборе песни открывают свой экран результатов.
- Настройки: разделы — кнопки со значками, панель уже.

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

