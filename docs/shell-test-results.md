# Shell test results

One section per run. Paste the report from the app as-is and add what you saw
by eye. How to run the test: `docs/testing.md`.

## Windows 10, WebView2 146, Tauri 2.12.6 · 17 Nov 2026 · automated half only

Release build, run unattended (`OUTPOST_SHELL_TEST`). Not yet watched by a
person, so tearing and white flashes are unassessed.

```
Outpost shell test
Date:     2026-11-18T03:08:08.302Z
Engine:   Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/146.0.0.0 Safari/537.36 Edg/146.0.0.0
Rows:     100,000   List height: 9,756,458 px
Window:   1320x860 @1x   Display interval: 6.1 ms (164 Hz)

phase                            frames  median     p95     p99   worst   slow  blank  longest blank
reading speed (600 px/s)            661     6.1     6.1     6.2     7.9      0      0     0.0 ms
fast wheel (6,000 px/s)             661     6.1     6.1     6.2     6.2      0      0     0.0 ms
scrollbar drag (120,000 px/s)       661     6.1     6.1     6.2     6.2      0      0     0.0 ms
back and forth                      661     6.1     6.1     6.2     6.2      0      1     6.0 ms
random jumps (5 per second)         661     6.1     6.2     6.4     7.1      0     19     6.2 ms

Slow frames: 0 of 3305 (0.0%).  Frames with a blank row: 20.
Times are ms per frame. 'slow' = more than 1.5 display intervals.
```

Reading: passes on timing. The 20 blank frames are single frames straight
after an instant jump to somewhere not yet loaded, which is the one case where
the data cannot already be there.

## WSL2 (Ubuntu 26.04, WSLg), WebKitGTK 2.52.6, Tauri 2.12.6 · 17 Nov 2026 · automated half only

Release build, run unattended. **This is not the real Linux test.** WSLg is a
remote-display layer, and in this run WebKit had no access to the graphics
card (`/dev/dri/renderD128: Permission denied`), so everything was drawn in
software. Not watched by a person.

```
Outpost shell test
Date:     2026-11-18T05:53:38.752Z
Engine:   Mozilla/5.0 (X11; Ubuntu; Linux x86_64) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/60.5 Safari/605.1.15
Rows:     100,000   List height: 9,668,690 px
Window:   1320x860 @1x   Display interval: 16.0 ms (63 Hz)

phase                            frames  median     p95     p99   worst   slow  blank  longest blank
reading speed (600 px/s)            242    17.0    17.0    18.0    21.0      0      0     0.0 ms
fast wheel (6,000 px/s)             220    18.0    19.0    20.0    21.0      0      0     0.0 ms
scrollbar drag (120,000 px/s)       223    18.0    19.0    20.0    21.0      0      0     0.0 ms
back and forth                      219    18.0    22.0    23.0    23.0      0      1    11.0 ms
random jumps (5 per second)         242    17.0    17.0    18.0    18.0      0     19    18.0 ms

Slow frames: 0 of 1146 (0.0%).  Frames with a blank row: 20.
Times are ms per frame. 'slow' = more than 1.5 display intervals.
```

Reading: no frame missed by more than half an interval and no blank rows
outside the jump phase, but the median frame is 17 to 18 ms against a 16 ms
display interval, so while scrolling fast it runs at about 55 frames a second
rather than a locked 63. On Windows the median equals the interval exactly.
Whether that gap is WebKitGTK or the software rendering cannot be told from
this run. The decision still needs a native Linux desktop with working
graphics, and the by-eye half.

### Follow-up runs, same machine, same day

After adding the user to the `render` and `video` groups the permission error
went away, but hardware graphics still did not start under WSLg in any
configuration tried (`libEGL warning: egl: failed to create dri2 screen`), so
every run here is software-rendered.

| Setting | Median frame while scrolling fast | Slow frames |
| --- | --- | --- |
| Default | 17 to 18 ms | 1 of 1,153 |
| `GALLIUM_DRIVER=d3d12` | 17 ms | 1 of 1,211 |
| `GDK_BACKEND=wayland GALLIUM_DRIVER=d3d12` | 17 ms | 2 of 1,210 |
| `WEBKIT_DISABLE_DMABUF_RENDERER=1` | 16 ms (matches the display) | 0 of 1,265 |

The best run, `WEBKIT_DISABLE_DMABUF_RENDERER=1`:

```
phase                            frames  median     p95     p99   worst   slow  blank  longest blank
reading speed (600 px/s)            249    16.0    17.0    17.0    17.0      0      0     0.0 ms
fast wheel (6,000 px/s)             249    16.0    17.0    17.0    17.0      0      0     0.0 ms
scrollbar drag (120,000 px/s)       249    16.0    17.0    17.0    17.0      0      0     0.0 ms
back and forth                      250    16.0    17.0    17.0    17.0      0      1    16.0 ms
random jumps (5 per second)         268    16.0    17.0    17.0    18.0      0     19    17.0 ms
Slow frames: 0 of 1265 (0.0%).  Frames with a blank row: 20.
```

Reading: with that one setting WebKitGTK keeps pace with the display even
without a graphics card, so the earlier shortfall came from how frames were
handed to the WSLg display, not from drawing the list. That is encouraging for
Tauri but is still a WSL result. Still needed: a native Linux desktop, and a
person watching.
