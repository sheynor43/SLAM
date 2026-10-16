common-button-ok = OK
common-button-cancel = Cancel
common-button-back = Back
settings-language-name = English
song-select-beatmap-count =
    { $count ->
        [one] { $count } beatmap
       *[other] { $count } beatmaps
    }
play-skip-button = Skip
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
