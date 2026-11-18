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

## Linux

Not run yet.
