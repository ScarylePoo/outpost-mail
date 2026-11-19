# Performance results

How to run these: `docs/testing.md`. Targets come from the handoff doc.

## Milestone 9 · 18 November 2026 · the release candidate, one million messages

Linux, 0.1.0 as it stands, the index rebuilt from the mail files and the
file cache emptied before measuring. The index's shape has not changed
since milestone 6 (schema 11), so this is a check that nothing drifted.
**All targets met.**

| Operation | First | Typical | Target |
| --- | --- | --- | --- |
| Open the account and list folders | 3 ms | 0 ms | - |
| Open Inbox (163 thousand) / Archive (352 thousand) | 49 / 34 ms | 9 / 18 ms | 100 ms |
| Open Inbox / Archive as conversations | 61 / 42 ms | 9 / 15 ms | 100 ms |
| Load a page at the middle / end of Archive | 11 / 9 ms | 4 / 8 ms | 16 ms |
| Read one message file | 0.1 ms | 0.0 ms | 100 ms |
| Sender history for one message | 70 ms | 26 ms | 100 ms |
| Search: word in nearly every message | 4 ms | 0.1 ms | 300 ms |
| Search: rare word | 19 ms | 0.1 ms | 300 ms |
| Search: common and rare word together | 21 ms | 0.5 ms | 300 ms |
| Search: three letters, as typed | 8 ms | 4 ms | 300 ms |
| Search: no matches | 0.5 ms | 0.0 ms | 300 ms |

Rebuild from the mail files: 259 s (reading and indexing 192 s at 5,219
messages a second, finishing 67 s). Index file 2.57 GB, 37% of the 6.88 GB
of mail.

"Sender history" has the least room: 70 ms cold against 100 ms.

**Windows**, the same mailbox on NTFS, measured straight after the rebuild
(so the file cache was warm: "first" here is not a cold read, and the tool
cannot empty the cache on Windows). **All targets met.**

| Operation | First | Typical | Target |
| --- | --- | --- | --- |
| Open the account and list folders | 13 ms | 0 ms | - |
| Open Inbox / Archive | 21 / 43 ms | 17 / 34 ms | 100 ms |
| Open Inbox / Archive as conversations | 25 / 33 ms | 18 / 22 ms | 100 ms |
| Load a page at the middle / end of Archive | 6 / 10 ms | 5 / 10 ms | 16 ms |
| Read one message file | 8 ms | 7 ms | 100 ms |
| Sender history for one message | 3 ms | 1 ms | 100 ms |
| Search, slowest kind (three letters, as typed) | 7 ms | 4 ms | 300 ms |

Rebuild on Windows: 639 s (reading and indexing 516 s at 1,939 messages a
second), two and a half times Linux, as milestone 1 found for anything that
touches many small files. Loading a page at the end of Archive, 10 ms
against 16 ms, has the least room.

Not measured in this run: scrolling frame rate in the app at this size
(last measured in milestone 3), and memory use.

## Milestone 6 · 18 November 2026 · sender verification, one million messages

Linux, file cache emptied first. The index gained a column and a table for
sender checks (schema 11). Nothing moved out of range:

| Operation | First | Typical | Target |
| --- | --- | --- | --- |
| Open Inbox / Archive | 41 / 36 ms | 10 / 22 ms | 100 ms |
| Open Inbox / Archive as conversations | 49 / 40 ms | 10 / 17 ms | 100 ms |
| Load a page at the middle / end of Archive | 10 / 9 ms | 4 / 8 ms | 16 ms |
| Search, word in nearly every message | 5 ms | 0.1 ms | 300 ms |
| Sender history for one message (new) | 60 ms | 27 ms | 100 ms |

Rebuild: 264 s. Index file 2.57 GB. "Sender history" is the lookup behind
the first-time, borrowed-name and lookalike warnings; it is added to the
time to open a message. Not re-measured on Windows for this milestone.

## Milestone 5 · 18 November 2026 · conversations, one million messages

Same mailbox and machine as below. The index now also holds conversations
(schema 8); the rebuild threads every message and compacts the file.

Linux, file cache emptied before the run ("first" is a truly cold read):

| Operation | First | Typical | Target |
| --- | --- | --- | --- |
| Open Inbox (162,790 messages) | 40 ms | 8.5 ms | 100 ms |
| Open Archive (352,194 messages) | 35 ms | 18.5 ms | 100 ms |
| Open Inbox as conversations (92,029) | 42 ms | 9.5 ms | 100 ms |
| Open Archive as conversations (200,206) | 65 ms | 15 ms | 100 ms |
| Search, word in nearly every message | 5 ms | 0.1 ms | 300 ms |
| Search, rare word | 17 ms | 0.1 ms | 300 ms |

Rebuild from the mail files: 285 s (212 s reading and indexing, 71 s
threading, ordering, building indexes and compacting). Index file 2.54 GB,
36.9% of the mail.

Before compacting was added, the cold figures for the same four opens were
147, 359, 124 and 218 ms: all missed. See the decision log.

Windows, measured straight after the rebuild, so the file cache was warm and
"first" is **not** a cold read here (a cold Windows figure needs a reboot
first, and has not been taken for this milestone):

| Operation | First | Typical | Target |
| --- | --- | --- | --- |
| Open Inbox | 13 ms | 10 ms | 100 ms |
| Open Archive | 27 ms | 21 ms | 100 ms |
| Open Inbox as conversations | 17 ms | 11 ms | 100 ms |
| Open Archive as conversations | 21 ms | 17 ms | 100 ms |
| Load a page at the middle / end of Archive | 5 / 10 ms | 5 / 9 ms | 16 ms |

Rebuild on Windows: 517 s (389 s reading and indexing, 115 s finishing).

## Milestone 1 · 17 November 2026 · one million messages

Same machine for both systems (32 logical CPUs, 16 GB given to WSL2). The
mailbox is `outpost-bench generate --messages 1000000 --seed 1`: 6.88 GB in
nine folders, the largest holding 352,194 messages.

### Summary

| Measure | Target | Windows | Linux (WSL2) | Linux, cold cache |
| --- | --- | --- | --- | --- |
| Open a 163,000-message folder | 100 ms | 13 ms | 12 ms | 45 ms |
| Open a 352,000-message folder | 100 ms | 25 ms | 24 ms | 72 ms |
| Search, worst case of ten kinds | 300 ms | 5 ms | 4 ms | 23 ms |
| Read one message file | 100 ms | 1.5 ms | 0.1 ms | 0.1 ms |
| Open the account (no scan) | part of the 2 s start | 2.5 ms | 1 ms | 3 ms |

"Open a folder" is everything the list needs before it can draw: the layout
of the whole folder plus the first 200 rows. Figures are first-run times; the
typical (repeat) times are lower and are in the full reports below.

**Every target is met on both systems.** Not covered here: cold-cache numbers
on Windows (the tool cannot empty the Windows file cache; the first run after
a reboot would show it), and the app's own start-up time.

### Rebuilding the index from the mail files

| | Windows | Linux (WSL2) |
| --- | --- | --- |
| Whole rebuild | 239 s | 234 s |
| Listing every folder | 0.8 s | 0.5 to 6 s |
| Listing the 352,000-file folder | 0.28 s | 0.2 to 2 s |
| Index file | 2.43 GB | 2.43 GB |

The index is 35% of this mailbox, but this mailbox is nearly all text. The
search index grows with the amount of text, not with attachments, so for a
real 80 GB mailbox expect a few GB, not 28 GB. To be measured on real mail.

### Writing mail files

| | Windows (NTFS) | Linux (ext4 in WSL2) |
| --- | --- | --- |
| Generating the million files | 2,565 s (390 a second) | 44 s (22,700 a second) |

Creating small files is about 60 times slower on Windows on this machine.
Microsoft Defender's real-time scanning was on and is the likely cause, but
that was not isolated. Reading the files back was not slow on either system.
Several hundred thousand files in one directory caused no problem in these
tests: listing 352,194 files took 0.28 s on Windows. The handoff doc's worry
about large directories on Windows is, so far, about creating files rather
than holding them.

### What changed during the milestone, and why

The first version met the targets warm on Linux but had two weak spots that
these tests exposed:

1. **A word found in nearly every message took 164 to 348 ms to search**
   (348 ms on Windows, over target), because every match was collected and
   sorted by date. Messages are now keyed so that the search index is already
   in date order; the same search takes about 1 ms. Cost: the index grew from
   1.72 GB to 2.43 GB.
2. **Opening a folder with nothing in the file cache took 2.8 s**, because a
   page of 200 rows was scattered across the index file, and because
   memory-mapped reads fault slowly. A rebuild now writes rows in list order
   and memory mapping is off: 45 ms.

### In the app

The Tauri app, release build, list fed from this index, showing the
352,194-message folder. Windows, WebView2, 164 Hz:

```
Outpost shell test
Date:     2026-11-18T07:27:00.256Z
Engine:   Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/146.0.0.0 Safari/537.36 Edg/146.0.0.0
Rows:     352,194   List height: 33,554,428 px
Window:   1320x860 @1x   Display interval: 6.1 ms (164 Hz)

phase                            frames  median     p95     p99   worst   slow  blank  longest blank
reading speed (600 px/s)            661     6.1     6.1     6.2     7.9      0      0     0.0 ms
fast wheel (6,000 px/s)             661     6.1     6.1     6.2     6.2      0      0     0.0 ms
scrollbar drag (120,000 px/s)       661     6.1     6.1     6.2     6.2      0      0     0.0 ms
back and forth                      661     6.1     6.1     6.2     6.2      0      2    12.2 ms
random jumps (5 per second)         661     6.1     6.3     6.4     6.6      0     40    18.2 ms

Slow frames: 0 of 3305 (0.0%).  Frames with a blank row: 42.
Times are ms per frame. 'slow' = more than 1.5 display intervals.
```

On Linux (WSLg, software rendering, `WEBKIT_DISABLE_DMABUF_RENDERER=1`) the
same run gave 0 slow frames of 1,265, and no blank rows outside the jump
phase, once the index was in the file cache.

**Known issue:** "List height: 33,554,428 px" is the engine's maximum element
height. This folder is slightly taller than that, so its oldest few thousand
messages cannot be scrolled to. To fix in milestone 3; see the decision log.

### Full reports

Windows:

```
Rebuilt index
  1000000 messages in 11 folders, 6.88 GB of mail, 0 unreadable
  total 239 s: listing 0.8 s, reading and indexing 195 s (5140 messages/s), finishing 43 s
  slowest directory listing: Archive, 352194 files in 0.28 s
  index file: 2.43 GB (35.3% of the mail)

Outpost store and index measurements
  1000000 messages, index file 2.43 GB, windows x86_64, 32 CPUs

  operation                                          first     typical    target  
  Open the account and list folders                 2.5 ms      0.0 ms         -         11 folders
  Open folder Inbox                                13.1 ms      8.8 ms    100 ms  ok     162790 messages: layout + first 200 rows
  Open folder Archive                              25.4 ms     18.9 ms    100 ms  ok     352194 messages: layout + first 200 rows
  Load a page at the middle of Archive              5.9 ms      5.2 ms     16 ms  ok     200 rows at offset 176097
  Load a page at the end of Archive                10.3 ms     15.0 ms     16 ms  ok     200 rows at offset 351994
  Read one message file                             1.5 ms      0.8 ms    100 ms  ok     locate in index + read from disk
  Search: in nearly every message                   1.4 ms      0.1 ms    300 ms  ok     "the" -> first 50
  Search: common word                               0.3 ms      0.1 ms    300 ms  ok     "let" -> first 50
  Search: ordinary word                             0.4 ms      0.1 ms    300 ms  ok     "ronbrulpaindprex" -> first 50
  Search: rare word                                 0.5 ms      0.1 ms    300 ms  ok     "stooljaindqueast" -> first 50
  Search: two common words                          0.3 ms      0.1 ms    300 ms  ok     "let other" -> first 50
  Search: common + rare word                        1.1 ms      0.6 ms    300 ms  ok     "let stooljaindqueast" -> first 50
  Search: 3-letter prefix, as typed                 5.1 ms      4.3 ms    300 ms  ok     "ron" -> first 50
  Search: a person's name                           0.2 ms      0.1 ms    300 ms  ok     "Bergström" -> first 50
  Search: common word, one folder                   0.3 ms      0.2 ms    300 ms  ok     "let" -> first 50
  Search: no matches                                0.1 ms      0.0 ms    300 ms  ok     "zzzxqj" -> first 0

  All targets met.
```

Linux, file cache emptied first:

```
Outpost store and index measurements
  1000000 messages, index file 2.43 GB, linux x86_64, 32 CPUs

  operation                                          first     typical    target  
  Open the account and list folders                 2.7 ms      0.0 ms         -         11 folders
  Open folder Inbox                                44.6 ms      8.0 ms    100 ms  ok     162790 messages: layout + first 200 rows
  Open folder Archive                              71.5 ms     17.1 ms    100 ms  ok     352194 messages: layout + first 200 rows
  Load a page at the middle of Archive             10.5 ms      4.4 ms     16 ms  ok     200 rows at offset 176097
  Load a page at the end of Archive                14.8 ms      8.7 ms     16 ms  ok     200 rows at offset 351994
  Read one message file                             0.1 ms      0.0 ms    100 ms  ok     locate in index + read from disk
  Search: in nearly every message                   6.6 ms      0.1 ms    300 ms  ok     "the" -> first 50
  Search: common word                               2.3 ms      0.1 ms    300 ms  ok     "let" -> first 50
  Search: ordinary word                             7.0 ms      0.1 ms    300 ms  ok     "ronbrulpaindprex" -> first 50
  Search: rare word                                12.2 ms      0.1 ms    300 ms  ok     "stooljaindqueast" -> first 50
  Search: two common words                          3.7 ms      0.1 ms    300 ms  ok     "let other" -> first 50
  Search: common + rare word                       19.2 ms      0.5 ms    300 ms  ok     "let stooljaindqueast" -> first 50
  Search: 3-letter prefix, as typed                23.0 ms      3.3 ms    300 ms  ok     "ron" -> first 50
  Search: a person's name                           2.0 ms      0.1 ms    300 ms  ok     "Bergström" -> first 50
  Search: common word, one folder                   1.4 ms      0.2 ms    300 ms  ok     "let" -> first 50
  Search: no matches                                0.5 ms      0.0 ms    300 ms  ok     "zzzxqj" -> first 0

  All targets met.
```

Linux, warm:

```
Outpost store and index measurements
  1000000 messages, index file 2.43 GB, linux x86_64, 32 CPUs

  operation                                          first     typical    target  
  Open the account and list folders                 0.8 ms      0.0 ms         -         11 folders
  Open folder Inbox                                10.7 ms      8.1 ms    100 ms  ok     162790 messages: layout + first 200 rows
  Open folder Archive                              23.2 ms     17.3 ms    100 ms  ok     352194 messages: layout + first 200 rows
  Load a page at the middle of Archive              4.5 ms      4.2 ms     16 ms  ok     200 rows at offset 176097
  Load a page at the end of Archive                 8.8 ms      8.4 ms     16 ms  ok     200 rows at offset 351994
  Read one message file                             0.1 ms      0.0 ms    100 ms  ok     locate in index + read from disk
  Search: in nearly every message                   0.4 ms      0.1 ms    300 ms  ok     "the" -> first 50
  Search: common word                               0.2 ms      0.1 ms    300 ms  ok     "let" -> first 50
  Search: ordinary word                             0.3 ms      0.1 ms    300 ms  ok     "ronbrulpaindprex" -> first 50
  Search: rare word                                 0.4 ms      0.1 ms    300 ms  ok     "stooljaindqueast" -> first 50
  Search: two common words                          0.2 ms      0.1 ms    300 ms  ok     "let other" -> first 50
  Search: common + rare word                        0.9 ms      0.5 ms    300 ms  ok     "let stooljaindqueast" -> first 50
  Search: 3-letter prefix, as typed                 4.5 ms      3.2 ms    300 ms  ok     "ron" -> first 50
  Search: a person's name                           0.2 ms      0.1 ms    300 ms  ok     "Bergström" -> first 50
  Search: common word, one folder                   0.2 ms      0.1 ms    300 ms  ok     "let" -> first 50
  Search: no matches                                0.0 ms      0.0 ms    300 ms  ok     "zzzxqj" -> first 0

  All targets met.
```

Windows, before the two changes above (first version of the index):

```
Rebuilt index
  1000000 messages in 11 folders, 6.88 GB of mail, 0 unreadable
  total 409 s: listing 0.6 s, reading and indexing 384 s (2605 messages/s), finishing 24 s
  slowest directory listing: Archive, 352194 files in 0.21 s
  index file: 1.72 GB (25.0% of the mail)

Outpost store and index measurements
  1000000 messages, index file 1.72 GB, windows x86_64, 32 CPUs

  operation                                          first     typical    target  
  Open the account and list folders                 2.6 ms      0.0 ms         -         11 folders
  Open folder Inbox                                 9.9 ms      9.3 ms    100 ms  ok     162790 messages: layout + first 200 rows
  Open folder Archive                              20.4 ms     19.8 ms    100 ms  ok     352194 messages: layout + first 200 rows
  Load a page at the middle of Archive              5.7 ms      5.1 ms     16 ms  ok     200 rows at offset 176097
  Load a page at the end of Archive                10.7 ms     10.6 ms     16 ms  ok     200 rows at offset 351994
  Read one message file                             0.7 ms      0.4 ms    100 ms  ok     locate in index + read from disk
  Search: in nearly every message                 348.0 ms    173.9 ms    300 ms  MISSED "the" -> first 50
  Search: common word                              78.5 ms     77.4 ms    300 ms  ok     "let" -> first 50
  Search: ordinary word                             5.9 ms      3.0 ms    300 ms  ok     "ronbrulpaindprex" -> first 50
  Search: rare word                                 0.6 ms      0.4 ms    300 ms  ok     "stooljaindqueast" -> first 50
  Search: two common words                         29.9 ms     25.7 ms    300 ms  ok     "let other" -> first 50
  Search: common + rare word                        0.9 ms      0.8 ms    300 ms  ok     "let stooljaindqueast" -> first 50
  Search: 3-letter prefix, as typed                22.0 ms     21.3 ms    300 ms  ok     "ron" -> first 50
  Search: a person's name                          48.1 ms     39.6 ms    300 ms  ok     "Bergström" -> first 50
  Search: common word, one folder                  64.6 ms     64.4 ms    300 ms  ok     "let" -> first 50
  Search: no matches                                0.1 ms      0.0 ms    300 ms  ok     "zzzxqj" -> first 0

  Some targets were MISSED.
```
