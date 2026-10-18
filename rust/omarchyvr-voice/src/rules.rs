//! Deterministic rules for VR-screen and session requests (live Work session 2026-10-16 15:49, 3 of ~16 right).
//! They run before the matcher's fuzzy step and the model, on the words after the wake word, and decide alone when
//! a request names VR screens by number, stops the session, or resizes screens: a small model dropped "to screen
//! 3" (gather on the left screen), turned "close screen number two" into closing a workspace's apps, and "remove
//! screen number four and three" into a swap. User rules (docs/VOICE.md, "Screens by voice"):
//! * "close / remove / hide / shut down screen N (and M ...)" HIDES those VR screens; no app ever closes;
//! * "close everything" / "close all the screens" hides every screen but the main one; no app closes;
//! * an app kind or name + "to screen N" moves those apps to screen N (`win.screen.put_n`);
//! * "make all (these) screens half the size / double / 75 percent" scales every screen;
//! * "stop / shut down / exit the VR / work / session" stops the session (always asks "yes" first);
//! * "move it / the screen in front of me" brings the screens back in front (`vr.screens.reset`).

use crate::command::Match;
use std::collections::BTreeMap;

const HIDE_VERBS: &[&str] = &["close", "closed", "closes", "closing", "shut", "shutdown", "remove", "removed", "hide", "kill", "delete", "drop", "lose"];
const STOP_VERBS: &[&str] = &["stop", "shut", "shutdown", "close", "closed", "exit", "quit", "end", "leave", "kill", "finish"];
const SESSION: &[&str] = &["vr", "work", "session"];
/// Words a stop-the-session request may have besides its verb and object.
const STOP_OK: &[&str] = &["down", "off", "turn", "switch", "whole", "virtual", "reality", "mode", "desk", "out", "of", "this", "it", "all", "now", "s", "sh", "st", "and", "everything", "headset"];
const MOVE_VERBS: &[&str] = &["move", "put", "send", "bring", "take", "throw", "place", "drag"];
/// App kinds and names a "put on screen N" can mean (the windows module resolves them).
const APP_WORDS: &[&str] = &[
    "terminal", "terminals", "console", "consoles", "shell", "shells", "foot", "kitty", "browser", "browsers", "chrome", "chromes", "chromium", "firefox", "brave", "web",
    "editor", "editors", "code", "files", "file", "spotify", "obsidian", "signal", "steam", "youtube", "pimax", "app", "apps", "everything", "music", "chat", "discord", "slack",
];
const SCREEN: &[&str] = &["screen", "screens", "pantalla", "pantallas"];
const DIRS: &[&str] = &["left", "right", "up", "down", "closer", "farther", "further", "bigger", "smaller"];

fn has(w: &[String], xs: &[&str]) -> bool {
    w.iter().any(|x| xs.contains(&x.as_str()))
}

fn m(id: &str, slots: &[(&str, String)]) -> Match {
    Match { id: id.into(), slots: slots.iter().map(|(k, v)| (k.to_string(), v.clone())).collect::<BTreeMap<_, _>>(), score: 0 }
}

/// The screen numbers named (1..6, in the order said, each once): "screens 4 and 3", "2, 3 and 5".
pub fn numbers(w: &[String]) -> Vec<u32> {
    let mut v = Vec::new();
    for x in w {
        if let Ok(n) = x.parse::<u32>() {
            if (1..=6).contains(&n) && !v.contains(&n) {
                v.push(n);
            }
        }
    }
    v
}

/// The words the rules read: fillers out, "number(s)" before a number out ("screen number three" = "screen 3").
pub fn tidy(w: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(w.len());
    for (i, x) in w.iter().enumerate() {
        let next_num = w.get(i + 1).is_some_and(|n| n.parse::<u32>().is_ok());
        // "no" / "num" only as the abbreviation after a screen word ("screen no. 3"); "two screens, no, three" is a
        // correction (command::take_back)
        let after_screen = i > 0 && matches!(w[i - 1].as_str(), "screen" | "screens" | "window" | "windows" | "workspace" | "pantalla");
        if (matches!(x.as_str(), "number" | "numbers") || (matches!(x.as_str(), "no" | "num") && after_screen)) && next_num {
            continue;
        }
        out.push(x.clone());
    }
    out
}

/// The size a resize asks for, in percent of now: "half" 50, "double" / "twice" 200, "a third" 33, "N percent" N.
fn size_of(w: &[String]) -> Option<u32> {
    for (i, x) in w.iter().enumerate() {
        match x.as_str() {
            "half" | "halve" | "halved" => return Some(50),
            "double" | "twice" | "doubled" => return Some(200),
            "third" => return Some(33),
            "quarter" => return Some(25),
            "percent" | "%" | "per" => {
                if let Some(n) = i.checked_sub(1).and_then(|j| w[j].parse::<u32>().ok()) {
                    return Some(n);
                }
            }
            _ => {
                if let Some(n) = x.strip_suffix('%').and_then(|n| n.parse::<u32>().ok()) {
                    return Some(n);
                }
            }
        }
    }
    None
}

/// The screen a side word names right before "screen" / "one" / "window" ("the left screen", "the upper one"): 1-6.
pub fn side_screen(w: &[String]) -> Option<usize> {
    w.windows(2).find(|p| crate::windows::slot_word(&p[0]).is_some() && matches!(p[1].as_str(), "screen" | "one" | "1" | "window")).and_then(|p| crate::windows::slot_word(&p[0])).and_then(crate::windows::screen_number)
}

/// The rule for these words (after the wake word), if one applies.
pub fn screen_rule(words: &[String]) -> Option<Match> {
    let w = tidy(words);
    let screen = has(&w, SCREEN);
    // an app named ("everything" moves with "to screen N", but "close everything" names no app)
    let apps = w.iter().any(|x| APP_WORDS.contains(&x.as_str()) && x != "everything");
    // screen numbers come after "screen(s)" / "window": a number before it is a count ("the two upper screens")
    let first_screen = w.iter().position(|x| SCREEN.contains(&x.as_str()) || matches!(x.as_str(), "window" | "windows" | "ventana"));
    let count_before = first_screen.is_some_and(|i| w[..i].iter().any(|x| x.parse::<u32>().is_ok()));
    let nums = first_screen.map(|i| numbers(&w[i..])).unwrap_or_default();
    let joined = format!(" {} ", w.join(" "));

    // resize: "make all these screens half the size", "make screen 2 double the size", "all screens 75 percent"
    if let Some(p) = size_of(&w).filter(|p| (10..=400).contains(p)) {
        if screen || has(&w, &["them", "all", "everything"]) {
            let which = if has(&w, &["all", "every", "these", "those", "each", "them", "everything"]) || w.iter().any(|x| x == "screens") {
                "all".to_string()
            } else if let Some(n) = nums.first() {
                n.to_string()
            } else {
                "here".to_string()
            };
            return Some(m("win.screen.scale", &[("screens", which), ("size", p.to_string())]));
        }
    }

    // hide VR screens by number ("remove screen number four and three", "shut down numbers screens 4 and 3",
    // "close down screen number two"); never an app
    // "high the left screen": the ASR's "hide" in front of a screen
    let high = w.windows(2).any(|p| p[0] == "high" && (matches!(p[1].as_str(), "the" | "screen" | "screens" | "this") || p[1].parse::<u32>().is_ok()));
    let hide_verb = high || has(&w, HIDE_VERBS) || joined.contains(" turn off ") || joined.contains(" switch off ") || joined.contains(" get rid of ");
    let badges = has(&w, &["numbers", "number", "badges", "badge"]) && nums.is_empty();
    // "close window 2": window = a VR screen in the user's words (docs/VOICE.md) when a number names it
    // "close the btop windows": a program name right before "windows" makes them apps, never VR screens
    let named_windows = w.windows(2).any(|p| {
        matches!(p[1].as_str(), "window" | "windows")
            && p[0].parse::<u32>().is_err()
            && !HIDE_VERBS.contains(&p[0].as_str())
            && !matches!(p[0].as_str(), "the" | "a" | "this" | "that" | "these" | "those" | "other" | "all" | "every" | "both" | "of" | "down" | "off" | "left" | "right" | "up" | "upper" | "top" | "main" | "middle" | "center" | "centre" | "first" | "second" | "third" | "last" | "extra" | "empty" | "my" | "your" | "two" | "three" | "four" | "five" | "six" | "number" | "out")
    });
    let screenish = screen || (has(&w, &["window", "windows", "ventana"]) && !nums.is_empty() && !named_windows);
    if hide_verb && screenish && !apps && !badges {
        // "close the other two screens": a count, not screen 2
        if has(&w, &["other", "others", "all", "every", "everything", "extra"]) {
            return Some(m("vr.screens.hide_others", &[]));
        }
        if count_before {
            return None;
        }
        if !nums.is_empty() {
            let list: Vec<String> = nums.iter().map(u32::to_string).collect();
            return Some(m("vr.screens.hide", &[("screens", list.join(","))]));
        }
        if w.iter().any(|x| x == "screens" || x == "pantallas") {
            return Some(m("vr.screens.hide_others", &[]));
        }
        // "hide the left screen", "switch off the upper one": a side names the screen
        let other_verb = w.iter().enumerate().any(|(i, x)| matches!(x.as_str(), "go" | "focus" | "open" | "move" | "put" | "make" | "show") || (x == "switch" && w.get(i + 1).is_none_or(|n| n != "off")));
        if let Some(n) = side_screen(&w).filter(|n| *n != 1 && !other_verb) {
            return Some(m("vr.screens.hide", &[("screens", n.to_string())]));
        }
        // "close this screen" (not "close this and go to the main screen")
        if w.windows(2).any(|p| (matches!(p[0].as_str(), "this" | "that") && SCREEN.contains(&p[1].as_str())) || (SCREEN.contains(&p[0].as_str()) && p[1] == "here")) {
            return Some(m("vr.screens.hide", &[("screens", "here".into())]));
        }
    }
    // "close everything", "close it all": the other screens hide, nothing closes
    if hide_verb && !apps && !screen {
        let rest: Vec<&String> = w.iter().filter(|x| !crate::command::FILLERS.contains(&x.as_str()) && !HIDE_VERBS.contains(&x.as_str())).collect();
        if !rest.is_empty() && rest.iter().all(|x| matches!(x.as_str(), "everything" | "all" | "it" | "them" | "down" | "of" | "up" | "stuff")) && has(&w, &["everything", "all"]) {
            return Some(m("vr.screens.hide_others", &[]));
        }
    }

    // stop the session: "shut down work VR", "can you stop the VR", "exit VR", "close work"
    let stop_verb = has(&w, STOP_VERBS) || joined.contains(" turn off ") || joined.contains(" switch off ");
    if stop_verb && has(&w, SESSION) && !screen && !apps {
        let ok = w.iter().all(|x| crate::command::FILLERS.contains(&x.as_str()) || STOP_VERBS.contains(&x.as_str()) || SESSION.contains(&x.as_str()) || STOP_OK.contains(&x.as_str()));
        if ok {
            return Some(m("vr.session.stop", &[]));
        }
    }

    // "move it / the screen in front of me": the screens back in front
    if joined.contains(" in front ") && (has(&w, MOVE_VERBS) || has(&w, &["get", "come"])) && !apps {
        return Some(m("vr.screens.reset", &[]));
    }

    // a VR screen moved in space: "move this screen number one to the right" (not an app: no app word)
    // ("screen 1", not "one of the screens")
    let named: Vec<u32> = w.windows(2).filter(|p| SCREEN.contains(&p[0].as_str())).filter_map(|p| p[1].parse::<u32>().ok()).filter(|n| (1..=6).contains(n)).collect();
    if has(&w, &["move", "push", "nudge", "shift"]) && !apps && named.len() == 1 && !has(&w, &["from", "swap"]) {
        if let Some(d) = w.iter().rev().find(|x| DIRS.contains(&x.as_str())) {
            let d = if d == "further" { "farther".to_string() } else { d.clone() };
            return Some(m("win.screen.adjust", &[("screen", named[0].to_string()), ("how", d)]));
        }
    }

    // a VR screen named by its side changes size or place: "make the left screen smaller", "the main one bigger",
    // "bring the right screen closer" (one screen: not "all", not the plural, no app)
    if !apps && !has(&w, &["from", "swap", "all", "every", "each", "screens", "these", "those"]) && named.is_empty() {
        let side_at = w.windows(2).position(|p| crate::windows::slot_word(&p[0]).is_some() && matches!(p[1].as_str(), "screen" | "one" | "1" | "window"));
        if let Some(i) = side_at {
            let how = w.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, x)| x.as_str()).find_map(|x| match x {
                "bigger" | "larger" => Some("bigger"),
                "smaller" => Some("smaller"),
                "closer" | "nearer" => Some("closer"),
                "farther" | "further" => Some("farther"),
                _ => None,
            });
            let n = crate::windows::slot_word(&w[i]).and_then(crate::windows::screen_number);
            if let (Some(how), Some(n)) = (how, n) {
                return Some(m("win.screen.adjust", &[("screen", n.to_string()), ("how", how.to_string())]));
            }
        }
    }

    // the screen itself moves: "move the screen up a bit", "push the window a little to the left" (no app, no screen
    // named by number; a side right before "screen" names a target screen instead: "move this to the left screen")
    if has(&w, &["move", "push", "nudge", "shift", "slide"]) && (screen || has(&w, &["window"])) && !apps && named.is_empty() && !has(&w, &["from", "swap", "all", "every", "screens"]) {
        let target_side = w.windows(2).any(|p| crate::windows::slot_word(&p[0]).is_some() && matches!(p[1].as_str(), "screen" | "one" | "1" | "window"));
        let dir = w.iter().find(|x| matches!(x.as_str(), "left" | "right" | "up" | "down" | "closer" | "farther" | "further" | "away"));
        if let (false, Some(d)) = (target_side, dir) {
            return Some(match d.as_str() {
                "closer" => m("win.screen.closer", &[]),
                "further" | "away" | "farther" => m("win.screen.farther", &[]),
                x => m("win.screen.nudge", &[("side", x.to_string())]),
            });
        }
    }

    // the screens are lost: "where did my screens go", "I can't see any window", "my screens disappeared"
    let lost = has(&w, &["gone", "lost", "disappeared", "vanished", "missing"]) || joined.contains(" where are ") || joined.contains(" where did ") || joined.contains(" where is ") || joined.contains(" where's ") || w.windows(2).any(|p| matches!(p[0].as_str(), "dont" | "don't" | "cant" | "can't" | "cannot" | "not") && p[1] == "see");
    if lost && (screen || has(&w, &["window", "windows"])) && !apps && nums.is_empty() && !has(&w, MOVE_VERBS) {
        return Some(m("vr.screens.reset", &[]));
    }

    // a screen wanted back: "show screen 5 again", "unhide the right one", "I need screen 3 back"
    let back = has(&w, &["unhide", "restore", "back", "again", "return"]) || (has(&w, &["show", "want", "need"]) && (nums.len() == 1 || side_screen(&w).is_some()));
    if back && screen && !apps && !hide_verb && !has(&w, MOVE_VERBS) && !has(&w, &["more", "another", "new", "add"]) {
        let n = if nums.len() == 1 { Some(nums[0] as usize) } else { side_screen(&w) };
        if let Some(slot) = n.filter(|n| (2..=6).contains(n)).and_then(|n| crate::windows::SCREEN_ORDER.get(n - 1)) {
            return Some(m("vr.screens.add", &[("side", slot.to_string())]));
        }
    }

    // to a screen named by its side: "move this to the right screen", "put the browsers on the upper screen"
    if has(&w, MOVE_VERBS) && !has(&w, &["from", "swap", "together"]) && nums.is_empty() {
        let at = w.windows(3).position(|p| matches!(p[0].as_str(), "to" | "on" | "onto" | "into") && (crate::windows::slot_word(&p[1]).is_some() && matches!(p[2].as_str(), "screen" | "one" | "1" | "window") || (p[1] == "the" && crate::windows::slot_word(&p[2]).is_some())));
        if let (Some(i), Some(n)) = (at, side_screen(&w)) {
            let v = w.iter().position(|x| MOVE_VERBS.contains(&x.as_str())).unwrap_or(0);
            if v < i {
                let obj: Vec<&str> = w[v + 1..i].iter().map(String::as_str).filter(|x| !crate::command::FILLERS.contains(x) && !matches!(*x, "all" | "of")).collect();
                if !obj.is_empty() && obj.iter().all(|x| matches!(*x, "this" | "it" | "that" | "app" | "window" | "program" | "1" | "one")) {
                    return Some(m("win.screen.move_n", &[("screen", n.to_string())]));
                }
                if !obj.is_empty() && obj.iter().any(|x| APP_WORDS.contains(x)) && !obj.contains(&"everything") {
                    return Some(m("win.screen.put_n", &[("app", obj.join(" ")), ("screen", n.to_string())]));
                }
            }
        }
    }

    // apps to a numbered screen: "move all the terminals to screen number three" (the last "screen N" counts)
    if has(&w, MOVE_VERBS) && has(&w, APP_WORDS) && !has(&w, &["from", "swap", "together", "one"]) {
        let target = w.windows(2).filter(|p| SCREEN.contains(&p[0].as_str()) || p[0] == "window").filter_map(|p| p[1].parse::<u32>().ok()).filter(|n| (1..=6).contains(n)).last();
        if let Some(n) = target {
            let v = w.iter().position(|x| MOVE_VERBS.contains(&x.as_str())).unwrap_or(0);
            let end = w[v + 1..].iter().position(|x| matches!(x.as_str(), "to" | "on" | "onto" | "into" | "in" | "over")).map(|e| v + 1 + e).unwrap_or(w.len());
            let obj: Vec<&str> = w[v + 1..end].iter().map(String::as_str).filter(|x| !matches!(*x, "the" | "all" | "of" | "my" | "a" | "uh" | "um")).collect();
            // "move this app to screen 3": the focused app
            if obj.iter().any(|x| matches!(*x, "this" | "it" | "that")) {
                return Some(m("win.screen.move_n", &[("screen", n.to_string())]));
            }
            if !obj.is_empty() && obj.iter().any(|x| APP_WORDS.contains(x)) {
                return Some(m("win.screen.put_n", &[("app", obj.join(" ")), ("screen", n.to_string())]));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(t: &str) -> Option<(String, Vec<(String, String)>)> {
        screen_rule(&crate::command::words(t)).map(|m| (m.id, m.slots.into_iter().collect()))
    }

    fn id(t: &str) -> String {
        rule(t).map(|x| x.0).unwrap_or_else(|| "-".into())
    }

    fn slot(t: &str, k: &str) -> String {
        rule(t).and_then(|x| x.1.into_iter().find(|(a, _)| a == k).map(|x| x.1)).unwrap_or_default()
    }

    #[test]
    fn live_session_2026_10_16_b() {
        assert_eq!(id("can you remove screen number four and three"), "vr.screens.hide");
        assert_eq!(slot("can you remove screen number four and three", "screens"), "4,3");
        assert_eq!(slot("can you shut down numbers screens four and three", "screens"), "4,3");
        assert_eq!(slot("hide screens 2, 3 and 5", "screens"), "2,3,5");
        for t in ["close down screen number two", "uh closed screen number two", "can you close screen number two", "it's close at screen number two"] {
            assert_eq!((id(t), slot(t, "screens")), ("vr.screens.hide".to_string(), "2".to_string()), "{t}");
        }
        assert_eq!(id("close everything"), "vr.screens.hide_others");
        assert_eq!(id("closed everything"), "vr.screens.hide_others");
        assert_eq!(id("close all the screens"), "vr.screens.hide_others");
        assert_eq!(id("close the other two screens"), "vr.screens.hide_others");
        assert_eq!(slot("close window 2", "screens"), "2");
        assert_eq!(id("can you shut down uh work vr"), "vr.session.stop");
        assert_eq!(id("can you stop the vr"), "vr.session.stop");
        assert_eq!(id("exit vr"), "vr.session.stop");
        assert_eq!(id("make all these screens half the size"), "win.screen.scale");
        assert_eq!((slot("make all of these screens half the size", "screens"), slot("make all of these screens half the size", "size")), ("all".into(), "50".into()));
        assert_eq!(slot("make all the screens double the size", "size"), "200");
        assert_eq!(slot("make all screens 75 percent", "size"), "75");
        assert_eq!(slot("make screen two half the size", "screens"), "2");
        assert_eq!(id("move all the terminals to number three to screen number three"), "win.screen.put_n");
        assert_eq!(slot("can you move all the terminals to screen number three", "screen"), "3");
        assert_eq!(slot("can you move all the terminals to screen number three", "app"), "terminals");
        assert_eq!(id("move it in front of me the screen"), "vr.screens.reset");
        assert_eq!(id("can you move this screen number one to the right"), "win.screen.adjust");
        assert_eq!(slot("can you move this screen number one to the right", "how"), "right");
        assert_eq!(id("can you move this app to screen three"), "win.screen.move_n");
    }

    #[test]
    fn general_grammar_2026_10_16() {
        // a side-named screen changes size or place; the screen itself moves; lost screens come back
        assert_eq!((id("make the upper screen larger"), slot("make the upper screen larger", "screen"), slot("make the upper screen larger", "how")), ("win.screen.adjust".into(), "4".into(), "bigger".into()));
        assert_eq!(slot("pull the right one nearer", "how"), "closer");
        assert_eq!((id("shift the window down slightly"), slot("shift the window down slightly", "side")), ("win.screen.nudge".into(), "down".into()));
        assert_eq!(id("push that screen away a little"), "win.screen.farther");
        assert_eq!(id("help, the windows vanished"), "vr.screens.reset");
        // a target screen is no direction; an app named moves the app
        assert_eq!((id("move this to the upper screen"), slot("move this to the upper screen", "screen")), ("win.screen.move_n".into(), "4".into()));
        assert_eq!((id("put the editors on the left one"), slot("put the editors on the left one", "screen")), ("win.screen.put_n".into(), "2".into()));
        assert_eq!((id("switch off the upper screen"), slot("switch off the upper screen", "screens")), ("vr.screens.hide".into(), "4".into()));
        assert_eq!((id("unhide the left screen"), slot("unhide the left screen", "side")), ("vr.screens.add".into(), "left".into()));
        assert_eq!(id("move the editor up"), "-");
    }

    #[test]
    fn never_for_other_requests() {
        for t in ["close the btop windows but not the htop one", "kill the btop windows", "close the chrome windows and spotify", "stop recording", "exit cinema mode", "close this window", "close this app", "close the terminals", "move all the terminals to one screen", "make the screen bigger", "move it to the right more", "turn off the passthrough", "close all apps", "stop the music", "what's on screen 2", "swap screens 1 and 2", "move chrome from screen 2 to screen 5", "hide the numbers on the screens", "hide the screen numbers", "can you move uh one of the screens to the right", "close this and go to the main screen", "get rid of the two upper screens"] {
            assert_eq!(id(t), "-", "{t}");
        }
    }
}
