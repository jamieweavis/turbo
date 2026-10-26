# Benchmarks

Jaycast compared with the three apps it replaces: **Raycast** (launcher, clipboard, snippets,
window management), **MonitorControl** (external display brightness over DDC) and **Mac Mouse Fix**
(smooth scrolling, mouse buttons).

Measured 2026-10-26 on an M1 Pro MacBook Pro with 16 GB RAM, macOS 27.0.1 (26A434). Jaycast is a
release build of `a860def` (`CONFIG=release just app`). All four apps ran at the same time with
their usual settings. Jaycast had every feature enabled: launcher, clipboard history, snippets,
smooth scrolling, mouse buttons, DDC brightness and the hyper key.

| App            | Version  |
| -------------- | -------- |
| Jaycast        | 0.1.0    |
| Raycast        | 2.9.0.0  |
| MonitorControl | 4.4.0    |
| Mac Mouse Fix  | 3.1.0    |

## Summary

|                         | Jaycast     | Raycast + MonitorControl + Mac Mouse Fix | Ratio        |
| ----------------------- | ----------- | ---------------------------------------- | ------------ |
| App bundle on disk      | **8.2 MB**  | 298.7 MB                                 | ~36× smaller |
| Memory at idle          | **22 MB**   | 259 MB                                   | ~12× less    |
| CPU at idle (3 min avg) | **0.42 %**  | 1.11 %                                   | ~2.6× less   |
| Processes               | **1**       | 6                                        |              |
| Threads                 | **15**      | ~85                                      |              |

## App size

`du -sk` of the `.app` bundle (1 MB = 1024 KB).

| App                  | Bundle       | Notes                                                              |
| -------------------- | ------------ | ------------------------------------------------------------------ |
| **Jaycast**          | **8.2 MB**   | One 8.6 MB executable (Rust core and Swift shell statically linked). 6.0 MB after `strip -S -x` |
| Raycast              | 243.9 MB     | 182 MB of that is `macos-app_RaycastDesktopApp.bundle`. Also bundles ripgrep, an updater and XPC services |
| MonitorControl       | 39.0 MB      | Ships its own Swift runtime dylibs (~15 MB) and Sparkle            |
| Mac Mouse Fix        | 15.8 MB      | Includes a login-item helper and Sparkle                           |
| **Three combined**   | **298.7 MB** |                                                                    |

## Memory

Physical footprint from `footprint -p <pid>`, the same number Activity Monitor shows as "Memory".
Peak is from `vmmap --summary`.

| Process                           | Idle (pass 1) | After pass 2 | Peak     | Threads |
| --------------------------------- | ------------- | ------------ | -------- | ------- |
| **Jaycast**                       | **22 MB**     | **31 MB**    | 37.8 MB  | 15–16   |
| Raycast (main)                    | 53 MB         | 53 MB        | 81.0 MB  | 26–28   |
| Raycast Backend (`node`)          | 160 MB        | 162 MB       | 239.0 MB | 37–38   |
| Raycast Accessibility XPC         | 7.0 MB        | 7.1 MB       | 7.2 MB   | 5       |
| Raycast Pasteboard XPC            | 6.8 MB        | 6.8 MB       | 7.1 MB   | 5–6     |
| *Raycast total*                   | *227 MB*      | *229 MB*     |          | *~76*   |
| MonitorControl                    | 18 MB         | 18 MB        | 19.0 MB  | 4       |
| Mac Mouse Fix Helper              | 14 MB         | 14 MB        | 15.9 MB  | 4–5     |
| **Three combined**                | **259 MB**    | **261 MB**   |          | **~85** |

Jaycast's whole footprint is smaller than Raycast's main process alone, and about the size of
MonitorControl and Mac Mouse Fix together (32 MB). The rise from 22 MB to 31 MB between passes
happened while the launcher was in use (see caveats).

## CPU

Average CPU over the window, from each process's CPU time (`ps -o time`) at the start and end.
100 % = one core fully busy.

| Process                    | Pass 1 (180 s)     | Pass 2 (300 s)     |
| -------------------------- | ------------------ | ------------------ |
| **Jaycast**                | **0.42 %** (0.76 s) | **0.38 %** (1.15 s) |
| Raycast (main)             | 0.52 % (0.93 s)    | 2.59 % (7.78 s)    |
| Raycast Backend (`node`)   | 0.26 % (0.47 s)    | 0.22 % (0.67 s)    |
| Raycast XPC services       | 0.06 % (0.10 s)    | 0.02 % (0.07 s)    |
| *Raycast total*            | *0.83 %*           | *2.84 %*           |
| MonitorControl             | 0.17 % (0.31 s)    | 0.13 % (0.38 s)    |
| Mac Mouse Fix Helper       | 0.10 % (0.18 s)    | 0.12 % (0.35 s)    |
| **Three combined**         | **1.11 %**         | **3.08 %**         |

Pass 1 is the cleaner idle number. In pass 2, Raycast's main process jumped to 7.8 s of CPU, which
suggests its window was opened during that pass.

### Where Jaycast's idle CPU goes

Jaycast uses more idle CPU than MonitorControl or Mac Mouse Fix on their own, but it also does the
idle work that Raycast does. `sample Jaycast 10` shows every thread parked in the kernel. The
regular wake-ups come from:

- **Clipboard polling** every 400 ms (`core/src/clipboard/mod.rs`, `POLL_INTERVAL`). macOS has no
  pasteboard change notification, so Raycast polls too, in its Pasteboard XPC service.
- **Accessibility permission polling** once a second (`AccessibilityPermission.swift`). This also
  catches the permission being revoked while the event taps are running.
- **Event taps** (smooth scroll, mouse buttons, hyper key, brightness keys), which run on every
  matching input event. Mac Mouse Fix does the same.

Making the clipboard poll slower, or skipping it while the system is idle, would cut the idle
cost the most.

## Caveats

- **Not a controlled lab run.** The machine was in normal use, and input during the windows
  wakes every event-tap app (Jaycast and Mac Mouse Fix). Pass 2 includes some use of the
  launchers.
- **Uptime differed.** Raycast, MonitorControl and Mac Mouse Fix had been running about 2 minutes
  when pass 1 started, and Jaycast about 20 seconds. All had been up 5 to 10 minutes by the end.
- **Raycast's numbers depend on its extensions.** The `node` backend's footprint grows with
  installed and recently run extensions.
- **Feature sets differ.** Raycast does far more than Jaycast's v1 scope (AI, store extensions,
  cloud sync). This compares the apps as installed, not feature for feature.
- **Load wasn't measured.** These numbers cover idle and light use only. Smooth-scroll CPU under a
  sustained scroll, and launcher open and search latency, need a scripted run with Mac Mouse Fix
  stopped, because both apps tap the same scroll events.

## Reproducing

```sh
CONFIG=release just run

# App sizes
du -sk build/Jaycast.app /Applications/{Raycast,MonitorControl}.app "/Applications/Mac Mouse Fix.app"

# Memory
for p in $(pgrep -f 'Jaycast.app|Raycast|Mac Mouse Fix|MonitorControl'); do footprint -p $p | grep Footprint; done

# CPU: take CPU time twice, N seconds apart; CPU % = Δtime / N
PIDS=$(pgrep -d, -f 'Jaycast.app|Raycast|Mac Mouse Fix|MonitorControl')
ps -o pid=,time=,comm= -p $PIDS; sleep 180; ps -o pid=,time=,comm= -p $PIDS
```
