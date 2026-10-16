# Voice decisions (user, 2026-10-15)

Source: analysis/voice-commands-review.html (questions 1-32). These override earlier docs where they differ.

1. "window" is always a whole VR screen; "this" or the app's name means one app. Old Omarchy "window" phrases (focus left, cycle windows) become "app".
2. "close window" asks "say yes" only when the screen has 2+ apps; one app or "close this" closes at once.
3. "move this left/right/up" moves the VR screen in space. An app goes to another screen with "send this to the left screen".
4. No voice in Play (games). Voice runs in Work only.
5. "put everything in its own window" sorts apps by kind (terminals together, browser, Pimax app, the rest); "put everything back together" merges them.
6. "go back" = undo the last voice action.
7. "zoom in/out" = bigger/smaller VR screen.
8. Sleep, log out, reboot and shut down are allowed by voice, each with "say yes".
9. Every other question: the recommended answer on the review page.

## 2026-10-16 (after the first live Work session with the local two-stage AI)

Source: the session's requests (voiceact.log, ai.log, voice.log) and the user's answers to the questions about them.

10. **Up to 6 VR screens**: main, left, right, up, **up-left** and **up-right** (above left and right). "Two more
    screens" / "two screens up there" add screens (up-left and up-right after the first four); "four screens" sets the
    count; "a screen to the left / up there" shows that one. 6 is the most. A screen the session lacks needs
    Screens = 6 in the app and a Work restart (WayVR reads its screens at start). docs/SCREENS.md.
11. **"Open / focus X in a new window"** moves X to a free VR screen of its own, which is focused. A browser tab moves
    alone (into its own new browser window there); another app moves whole; X not open: it opens there.
12. **"Move all the terminals to one window"** (also "command line", "browsers", an app name): every app of that kind
    joins one screen (the one in use); the other apps stay where they are.
13. **"Close (the other) screens"** shows the main screen only (work-1); no app closes.
14. **"Move one of the screens to the right"** moves the VR screen itself in space (win.screen.nudge), not an app.
15. **"The main window"** = the main (middle) VR screen ("focus in the main window" = go to the main screen).
16. **"Stop passthrough"** (heard "stop us through", "past through") = passthrough off (black).
17. **"Max" anywhere**: when the speech-to-text marks a pause before or after "Max" ("Windows, can we ha Max, can we
    have two more windows?"), what follows "Max" runs. "I told Max to switch" (no pause) still never triggers.
18. **Never type fillers**, and wait for the end of the sentence: "uh"/"um" are dropped from typed text and dictation; a
    sentence cut off mid-way ("can you write uh something in the") waits for its rest instead of running.
19. An app slot is never the user's name or a word that was not heard: "can I focus you 2" asks "Did you mean
    "youtube"?" (the closest open app or site to the words) instead of focusing a terminal by its title.
