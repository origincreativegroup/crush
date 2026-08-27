# TASK-012c: Search + Shot detail  ⛔ HARD STOP AFTER
Agent: Cursor on the Mac. Branch: task/12c-search. Depends: 012b. UX spec: docs/ux-spec.md.

## Acceptance
- [ ] Search box focused on launch and on Cmd-F; results grid of thumbnails (score, duration, filename) within 500 ms for 5k shots
- [ ] Shot detail: `<video>` element playing the source file from start_s (Tauri asset protocol), stops at end_s; timecodes in `HH:MM:SS.ff`; transcript snippet; Copy path+timecodes; Export clip (save dialog); Reveal in Finder
- [ ] "No matches" and "nothing indexed" states
- [ ] Dark theme, monospace timecodes, no layout jank while thumbnails load (fixed aspect boxes)

## Human review
**John uses it for ten minutes on real footage and writes every annoyance into docs/smoke.md before Task 13.**
