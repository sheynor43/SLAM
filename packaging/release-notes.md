> **Alpha build for testers.** Many features are missing and crashes are possible. Русская версия — ниже.

## English

### What's new in 0.1.3
**Mods**
- New mod select panel in lazer's style: columns by mod type in the lower part of the screen, each mod with its name and description; the song list stays visible behind it.
- **Autoplay**, **Relax** and **Autopilot**. Autoplay moves the cursor smoothly from object to object, as stable does. Autoplay plays keep neither a replay nor a score.
- Mod hotkeys can work as in lazer (by place in the column) or as in stable (a letter per mod: Q Easy, W No Fail, E Half Time, A Hard Rock, S Sudden Death, D Double Time, F Hidden, Z Relax, V Autoplay; press again for Daycore, Perfect, Nightcore or Traceable, Shift goes back): **Settings → Gameplay → Mod hotkeys**.

**Song select**
- Difficulty rows show a star counter, the star rating and the maximum pp with the selected mods (**Settings → Gameplay → Difficulty row shows**).
- Set and difficulty rows look and move as in lazer: their heights can be set, the selected row is centred.
- The info panel at the top left changes smoothly to another difficulty.
- Long names that do not fit scroll back and forth.

**Interface**
- A volume overlay: Alt + mouse wheel anywhere, Alt+↑/↓, Ctrl+M to mute; its style and screen edge are in the settings.
- A new interface font with Cyrillic, Onest; smaller captions and hints.
- After a replay is saved, a notification appears; click it to show the file.

**Known limitations**
- Hot-reloading skins, skin profiles, `.osk` import and choosing HUD layouts in the settings are not done yet.
- The skin's score and accuracy sit 10–13 px further right than in lazer and stable at 1920×1080.
- Search keys `star`, `bpm`, `length`, `divisor`, `status` and dates are accepted but do not filter yet.
- Spinners follow lazer's rules, not stable's.
- With Autoplay or Relax the key overlay counts your own keys, not the game's presses.
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

### Что нового в 0.1.3
**Моды**
- Новая панель модов в стиле лазера: колонки по типам модов в нижней части экрана, у каждого мода название и описание; список карт остаётся виден за ней.
- **Autoplay**, **Relax** и **Autopilot**. Autoplay плавно ведёт курсор от объекта к объекту, как в stable. Игры с Autoplay не сохраняют ни реплей, ни рекорд.
- Горячие клавиши модов работают как в лазере (по месту в колонке) или как в stable (у мода своя буква: Q Easy, W No Fail, E Half Time, A Hard Rock, S Sudden Death, D Double Time, F Hidden, Z Relax, V Autoplay; повторное нажатие — Daycore, Perfect, Nightcore или Traceable, Shift — назад): **Настройки → Игра → Клавиши модов**.

**Выбор песни**
- В строке сложности — счётчик звёзд, звёзды и максимум pp с выбранными модами (**Настройки → Игра → В строке сложности**).
- Строки сетов и сложностей выглядят и двигаются как в лазере: их высоту можно настроить, выбранная строка встаёт по центру.
- Панель сведений слева вверху плавно меняется при смене сложности.
- Длинные названия, которые не помещаются, прокручиваются туда-обратно.

**Интерфейс**
- Оверлей громкости: Alt + колесо мыши везде, Alt+↑/↓, Ctrl+M — выключить звук; вид и край экрана — в настройках.
- Новый шрифт интерфейса с кириллицей, Onest; подписи и подсказки мельче.
- После сохранения реплея появляется уведомление; по клику открывается файл.

**Известные ограничения**
- Горячей перезагрузки скинов, профилей скина, импорта `.osk` и выбора раскладки HUD в настройках пока нет.
- Счёт и точность скина при 1920×1080 стоят на 10–13 px правее, чем в лазере и stable.
- Ключи поиска `star`, `bpm`, `length`, `divisor`, `status` и даты принимаются, но пока не фильтруют.
- Спиннеры работают по правилам лазера, а не stable.
- С Autoplay и Relax оверлей клавиш считает ваши клавиши, а не нажатия игры.
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

