common-button-ok = OK
common-button-cancel = Cancel
common-button-back = Back
settings-language-name = English
settings-audio-master-volume = Master
settings-audio-music-volume = Music
settings-audio-hitsound-volume = Hitsounds
settings-audio-effects-volume = Effects
settings-keybind-listening = Press a key...
song-select-beatmap-count =
    { $count ->
        [one] { $count } beatmap
       *[other] { $count } beatmaps
    }
play-skip-button = Skip
play-offset = { $offset } ms
play-offset-earlier = { $offset } ms (hit objects appear earlier)
play-offset-later = { $offset } ms (hit objects appear later)
menu-title = SLAM
menu-play-button = Play
menu-settings-button = Settings
menu-exit-button = Exit
song-select-no-folder = No songs folder. Start SLAM with --songs <folder>.
song-select-scanning =
    { $count ->
        [one] Scanning songs: { $count } set found so far
       *[other] Scanning songs: { $count } sets found so far
    }
song-select-failed = Could not read the songs folder.
song-select-empty = No beatmaps found in the songs folder.
song-select-loading = Loading beatmap...
song-select-hint = Enter: play · Esc: back
results-score-title = Score
results-accuracy-title = Accuracy
results-max-combo-title = Max combo
results-mods-title = Mods
results-no-mods = None
results-great-label = Great
results-ok-label = Ok
results-meh-label = Meh
results-miss-label = Miss
results-slider-tick-label = Slider tick
results-slider-end-label = Slider end
results-spinner-spin-label = Spinner spin
results-spinner-bonus-label = Spinner bonus
results-retry-button = Retry
results-back-button = Back
results-hint = R: retry · Esc: back
settings-title = Settings
settings-section-audio = Audio
settings-section-offset = Offset
settings-section-keys = Keys
settings-section-frame = Frame rates
settings-section-gameplay = Gameplay
settings-restart-marker = after restart
settings-unit-percent = %
settings-unit-ms = { " " }ms
settings-option-default = Default
settings-option-unlimited = Unlimited
settings-option-hz = { $value } Hz
settings-option-frames = { $value } frames
settings-audio-backend = Audio backend
settings-audio-buffer = Buffer size
settings-offset-global = Global offset
settings-keys-mouse-buttons = Mouse buttons in gameplay
settings-frame-update-rate = Update rate
settings-frame-draw-rate = Gameplay frame rate
settings-frame-menu-draw-rate = Menu frame rate
settings-gameplay-dim = Background dim
settings-gameplay-beatmap-colours = Beatmap colours
