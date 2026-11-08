# File associations and extra windows

For the installer (written later) and for anyone testing by hand.

## `.dbrn` projects

The app opens any file given on the command line once its window is up: `darkburn.exe "C:\path\Design.dbrn"`.
Projects open as the current project (the app asks to save changes first when needed); other artwork
(DXF, PDF, AI, PLT, SVG, pictures, LightBurn files) is read in the background.

Registry entries for the installer (per user, so no admin rights are needed):

```
HKCU\Software\Classes\.dbrn                                  (Default) = DarkBurn.Project
HKCU\Software\Classes\DarkBurn.Project                       (Default) = DarkBurn project
HKCU\Software\Classes\DarkBurn.Project\DefaultIcon           (Default) = "<install dir>\darkburn.exe",0
HKCU\Software\Classes\DarkBurn.Project\shell\open\command    (Default) = "<install dir>\darkburn.exe" "%1"
```

`.dbdev` (device profiles) can use the same pattern with their own ProgID; the app hands a device profile
it is given to the devices code (`FileIo::device_profiles`).

## More than one window

- `File > New Window` starts another copy with `--secondary`. A secondary copy has its own project and leaves the
  settings folder's session files (auto-save, crash marker, running marker) to the first copy.
- With `Settings > Files > Only one DarkBurn at a time` on, starting DarkBurn again (from Explorer, a file
  association) hands the files to the running copy and exits; `File > New Window` then just starts a new empty
  project in the one window. The running copy writes
  `instance.alive` into the settings folder every second; a launch that finds it fresher than five seconds drops
  the paths into `inbox\` and quits. No sockets or pipes are used.
