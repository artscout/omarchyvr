//! Tests of the automatic setup's state machine: fake checks and a fake root job, no machine, no pkexec.
use super::*;
use crate::checklist::RootFix;

fn chk(id: &str, state: CheckState, fix: CheckFix) -> Check {
    Check { id: id.into(), title: id.into(), state, reason: if state == CheckState::Ok { String::new() } else { format!("{id} is wrong") }, detail: String::new(), fix }
}
fn ok(id: &str) -> Check {
    chk(id, CheckState::Ok, CheckFix::None)
}
fn bad(id: &str, fix: CheckFix) -> Check {
    chk(id, CheckState::Fail, fix)
}
fn waits(id: &str) -> Check {
    chk(id, CheckState::WaitsForRestart, CheckFix::None)
}
fn all_green() -> Vec<Check> {
    ["headset", "udev-rule", "keepalive", "modinfo", "merged-head-sync", "connector", "setcap-desk"].iter().map(|i| ok(i)).collect()
}

const PLAN: &str = "omarchyvr system setup (plan only, nothing changes):\n 1. Restore point: a snapper snapshot of /\n      would: snapper -c root create\n 2. Headset access: udev rule /etc/udev/rules.d/70-pimax.rules (only Pimax devices)\n 3. Uninstall helper: /usr/local/lib/omarchyvr/omarchyvr-root.sh (root-owned copy)\n 4. NVIDIA 610.57.04 (tested): display patches for the headset's two-head mode\n      would: patch /usr/src/nvidia-610.57.04, dkms build + install\n 5. Headset display mode at boot: EDID override (connector found after the headset wakes, up to 30 s)\n 6. Realtime priority for the VR compositor: cap_sys_nice on /home/u/monado-service (once; Play never asks again)\nREBOOT=no\n";
const APPLY_DONE: &str = "omarchyvr system setup (applying):\n 1. Restore point: a snapper snapshot of /\n      snapper -c root create -d 'before omarchyvr install'\n 2. Headset access: udev rule /etc/udev/rules.d/70-pimax.rules (only Pimax devices)\n      wrote /etc/udev/rules.d/70-pimax.rules\n 3. Uninstall helper: /usr/local/lib/omarchyvr/omarchyvr-root.sh (root-owned copy)\n      wrote /usr/local/lib/omarchyvr/omarchyvr-root.sh\n 4. NVIDIA 610.57.04 (tested): display patches for the headset's two-head mode\n      patch /usr/src/nvidia-610.57.04\n 5. Headset display mode at boot: EDID override (connector found after the headset wakes, up to 30 s)\n      headset display found: [0000:01:00.0].DP-0\n 6. Realtime priority for the VR compositor: cap_sys_nice on /home/u/monado-service (once; Play never asks again)\n      setcap cap_sys_nice+ep /home/u/monado-service\nREBOOT=yes\n";

fn fresh() -> AutoSetup {
    let mut s = AutoSetup::default();
    s.set_plan(PLAN);
    s
}

#[test]
fn the_progress_of_the_root_job_is_parsed_into_a_live_list() {
    let steps = parse_steps(PLAN);
    assert_eq!(steps.len(), 6);
    assert!(steps.iter().all(|s| s.state == StepState::Pending));
    assert_eq!(steps[0].title, "Save a restore point of your system");
    assert_eq!(steps[1].title, "Let your user open the headset");
    assert_eq!(steps[3].title, "Patch the NVIDIA driver for the headset");
    assert_eq!(steps[4].title, "Set the headset's display mode for the next start");
    assert_eq!(steps[5].title, "Allow realtime priority for VR");
    // half way: step 4 runs, 1-3 are done, 5-6 wait
    let half: String = APPLY_DONE.lines().take(9).collect::<Vec<_>>().join("\n");
    let mut s = steps.clone();
    apply_progress(&mut s, &half);
    let st: Vec<StepState> = s.iter().map(|x| x.state).collect();
    assert_eq!(st, vec![StepState::Done, StepState::Done, StepState::Done, StepState::Running, StepState::Pending, StepState::Pending]);
    assert!(s[1].note.is_empty(), "'wrote ...' lines are not notes");
    // the whole output: the last step still runs until the job ends
    let mut s = steps;
    apply_progress(&mut s, APPLY_DONE);
    assert_eq!(s[5].state, StepState::Running);
    assert_eq!(s[4].state, StepState::Done);
    assert_eq!(s[4].note, "headset display found: [0000:01:00.0].DP-0");
    // a step the job never prints is skipped when it ends
    let mut v = vec![RootStep { n: 7, title: "x".into(), raw: "x".into(), state: StepState::Pending, note: String::new() }];
    finish_steps(&mut v, false);
    assert_eq!(v[0].state, StepState::Skipped);
}

#[test]
fn skipped_steps_say_why_in_plain_words() {
    let mut s = Vec::new();
    apply_progress(
        &mut s,
        " 4. NVIDIA 610.58.01 (NOT tested): the patch is NOT applied; the stock module stays (the desktop works, the headset picture will not)\n 5. Headset display mode at boot: EDID override (connector found after the headset wakes, up to 30 s)\n      headset display not seen: put the headset on / turn it on, then press Fix in the app (EDID override skipped, nothing else is affected)\n 6. NVIDIA: no driver patch (not needed or not a tested version; see the installer's message)\n",
    );
    assert_eq!(s[0].state, StepState::Skipped);
    assert_eq!(s[0].title, "NVIDIA driver (version not tested: not patched)");
    assert!(s[0].note.contains("not tested"));
    assert_eq!(s[1].state, StepState::Skipped);
    assert!(s[1].note.contains("turn the headset on"));
    assert_eq!(s[2].state, StepState::Skipped);
    assert_eq!(friendly("Something new: with detail"), "Something new");
}

#[test]
fn exit_codes_become_plain_outcomes() {
    let e = |c: u32| Err(format!("`pkexec ...` failed (exit code {c})"));
    assert_eq!(outcome_of(&e(126), ""), RootOutcome::Cancelled);
    assert_eq!(outcome_of(&e(127), ""), RootOutcome::WrongPassword);
    assert_eq!(outcome_of(&Ok(()), APPLY_DONE), RootOutcome::Ok { reboot: true });
    assert_eq!(outcome_of(&Ok(()), "REBOOT=no\n"), RootOutcome::Ok { reboot: false });
    assert_eq!(outcome_of(&e(3), "      headset display not seen: put the headset on\nREBOOT=no\n"), RootOutcome::NoHeadsetDisplay);
    match outcome_of(&e(1), " 4. NVIDIA\nrefusing setcap: not the user's monado-service\n") {
        RootOutcome::Failed(t) => assert!(t.starts_with("refusing setcap"), "{t}"),
        o => panic!("{o:?}"),
    }
}

#[test]
fn all_green_with_a_restart_needed() {
    let mut s = fresh();
    assert_eq!(s.start(RunKind::Fresh, true), Action::RunChecks);
    assert_eq!(s.phase, Phase::Looking);
    // stage 1 finds the keep-alive (no password) and the udev rule (root)
    let a = s.checks_done(vec![ok("headset"), bad("keepalive", CheckFix::User), bad("udev-rule", CheckFix::Root { reboot: false })]);
    assert_eq!(a, Action::RunUserFix("keepalive".into()));
    assert_eq!((s.phase.clone(), s.stages[0], s.stages[1]), (Phase::FixingUser, StepState::Done, StepState::Running));
    // stage 2 done -> stage 3: the whole root job, ONE password
    assert_eq!(s.user_fix_done(true), Action::RunRoot(RootPlan::Setup));
    assert_eq!((s.phase.clone(), s.stages[1], s.stages[2]), (Phase::Password, StepState::Done, StepState::Running));
    s.root_output(&APPLY_DONE.lines().take(9).collect::<Vec<_>>().join("\n"));
    assert_eq!(s.current_step().map(|x| x.n), Some(4));
    s.root_output(APPLY_DONE);
    assert!(s.reboot, "REBOOT=yes seen while it runs");
    assert_eq!(s.root_finished(RootOutcome::Ok { reboot: true }), Action::RunChecks);
    assert_eq!(s.phase, Phase::Verifying);
    assert!(s.root_steps.iter().all(|x| x.state == StepState::Done), "{:?}", s.root_steps);
    // stage 4: everything green, the patched module waits for the restart
    let mut c = all_green();
    c.retain(|x| x.id != "merged-head-sync");
    c.push(waits("merged-head-sync"));
    assert_eq!(s.checks_done(c), Action::None);
    assert_eq!(s.phase, Phase::NeedsRestart);
    assert_eq!(s.stage(), Some(SetupStage::Restart));
    assert_eq!(s.stages, [StepState::Done; 4]);
}

#[test]
fn nothing_to_fix_and_no_root_job_means_no_password_and_ready() {
    let mut s = AutoSetup::default();
    s.start(RunKind::Fresh, false);
    assert_eq!(s.checks_done(all_green()), Action::None);
    assert_eq!(s.phase, Phase::Ready);
    assert_eq!(s.stages, [StepState::Done, StepState::Skipped, StepState::Skipped, StepState::Done]);
    assert_eq!(s.stage(), Some(SetupStage::Done));
    assert!(!s.reboot);
}

#[test]
fn the_root_job_ran_and_asked_for_a_restart_without_a_waiting_check() {
    let mut s = fresh();
    s.start(RunKind::Fresh, true);
    s.checks_done(all_green());
    assert_eq!(s.stages[1], StepState::Skipped);
    s.root_finished(RootOutcome::Ok { reboot: true });
    s.checks_done(all_green());
    assert_eq!(s.phase, Phase::NeedsRestart, "REBOOT=yes alone is enough");
}

#[test]
fn a_root_step_fails_the_failure_is_listed_with_a_retry_and_the_rest_is_still_checked() {
    let mut s = fresh();
    s.start(RunKind::Fresh, true);
    s.checks_done(all_green());
    s.root_output(" 1. Restore point: a snapper snapshot of /\n 2. Headset access: udev rule\n 4. NVIDIA 610.57.04 (tested): display patches\n");
    assert_eq!(s.current_step().map(|x| x.n), Some(4));
    let a = s.root_finished(RootOutcome::Failed("the patches do not apply to this source (exit code 1)".into()));
    assert_eq!(a, Action::RunChecks, "what is still fine is checked anyway");
    let failed: Vec<u32> = s.root_steps.iter().filter(|x| x.state == StepState::Failed).map(|x| x.n).collect();
    assert_eq!(failed, vec![4]);
    assert_eq!(s.stages[2], StepState::Failed);
    s.checks_done(vec![ok("headset"), bad("modinfo", CheckFix::Root { reboot: true })]);
    assert_eq!(s.phase, Phase::Problems);
    assert_eq!(s.problems[0].id, "root-job", "the failed job is the first problem");
    assert!(s.problems[0].reason.contains("do not apply"));
    // its Fix runs the whole job again (still one password)
    assert_eq!(s.fix_one("root-job"), Action::RunRoot(RootPlan::Setup));
    assert_eq!(s.phase, Phase::Password);
    assert!(s.root_error.is_none());
}

#[test]
fn pkexec_cancelled_or_a_wrong_password_stop_with_a_plain_message_and_try_again() {
    for (outcome, stop, words) in [(RootOutcome::Cancelled, Stop::Cancelled, "closed"), (RootOutcome::WrongPassword, Stop::WrongPassword, "not accepted")] {
        let mut s = fresh();
        s.start(RunKind::Fresh, true);
        s.checks_done(all_green());
        assert_eq!(s.root_finished(outcome), Action::None);
        assert_eq!(s.phase, Phase::PasswordStopped(stop));
        assert!(s.headline().contains(words), "{}", s.headline());
        assert!(s.headline().contains("Nothing was changed"));
        assert_eq!(s.stages[2], StepState::Failed);
        // Try again: the whole run, the root job again
        assert_eq!(s.retry(), Action::RunChecks);
        assert_eq!(s.phase, Phase::Looking);
        assert_eq!(s.checks_done(all_green()), Action::RunRoot(RootPlan::Setup));
    }
    let mut s = fresh();
    s.start(RunKind::Fresh, true);
    s.checks_done(all_green());
    s.no_password_dialog();
    assert_eq!(s.phase, Phase::PasswordStopped(Stop::NoDialog));
}

#[test]
fn after_the_restart_the_checks_continue_by_themselves_and_end_in_ready() {
    let mut s = AutoSetup::default();
    assert_eq!(s.start(RunKind::AfterReboot, false), Action::RunChecks);
    assert_eq!((s.phase.clone(), s.stages), (Phase::Verifying, [StepState::Done, StepState::Done, StepState::Done, StepState::Running]));
    assert_eq!(s.headline(), "Checking after the restart...");
    s.checks_done(all_green());
    assert_eq!(s.phase, Phase::Ready);
    assert_eq!(s.stage(), Some(SetupStage::Done));
    // the restart did not happen yet: the module is still not loaded -> the restart screen again
    let mut s = AutoSetup::default();
    s.start(RunKind::AfterReboot, false);
    s.checks_done(vec![ok("headset"), waits("merged-head-sync")]);
    assert_eq!(s.phase, Phase::NeedsRestart);
}

#[test]
fn the_edid_override_missing_after_the_restart_is_one_fix_and_needs_another_restart() {
    let mut s = AutoSetup::default();
    s.start(RunKind::AfterReboot, false);
    s.checks_done(vec![ok("headset"), ok("merged-head-sync"), bad("connector", CheckFix::Root { reboot: true }), ok("setcap-desk")]);
    assert_eq!(s.phase, Phase::Problems);
    assert_eq!(s.problems.len(), 1);
    assert_eq!(s.problems[0].id, "connector");
    assert_eq!(s.stages[3], StepState::Failed);
    assert_eq!(s.fix_one("connector"), Action::RunRoot(RootPlan::Fixes(vec!["connector".into()])));
    assert_eq!(s.root_finished(RootOutcome::Ok { reboot: true }), Action::RunChecks);
    // the override is written, the display mode takes effect after the next restart
    s.checks_done(vec![ok("headset"), ok("merged-head-sync"), waits("connector"), ok("setcap-desk")]);
    assert_eq!(s.phase, Phase::NeedsRestart);
    // the headset display was not visible: only that step is reported, with the same Fix
    let mut s = AutoSetup::default();
    s.start(RunKind::AfterReboot, false);
    s.checks_done(vec![bad("connector", CheckFix::Root { reboot: true })]);
    s.fix_one("connector");
    assert_eq!(s.root_finished(RootOutcome::NoHeadsetDisplay), Action::RunChecks);
    s.checks_done(vec![bad("connector", CheckFix::Root { reboot: true })]);
    assert_eq!(s.phase, Phase::Problems);
    assert_eq!(s.problems[0].id, "connector", "the display-mode check carries the Fix, not the whole job");
    assert!(s.problems[0].reason.contains("display did not show up"), "{}", s.problems[0].reason);
    assert_eq!(s.fix_one("connector"), Action::RunRoot(RootPlan::Fixes(vec!["connector".into()])), "only the EDID step again");
    // the full job whose EDID step was skipped: no root-job problem, and the EDID Fix once the display shows up
    let mut s = AutoSetup::default();
    s.set_plan("OVR_X=1\n");
    s.start(RunKind::Fresh, true);
    s.checks_done(vec![ok("headset")]);
    assert!(matches!(s.phase, Phase::Password));
    assert_eq!(s.root_finished(RootOutcome::NoHeadsetDisplay), Action::RunChecks);
    s.checks_done(vec![ok("headset"), bad("connector", CheckFix::None)]);
    assert_eq!(s.phase, Phase::Problems);
    assert!(s.problems.iter().all(|p| p.id != "root-job"));
    assert_eq!(s.fix_one("connector"), Action::None, "the display is not there yet: Check again");
    assert_eq!(s.check_again(), Action::RunChecks);
    s.checks_done(vec![ok("headset"), bad("connector", CheckFix::Root { reboot: true })]);
    assert_eq!(s.fix_all(), Action::RunRoot(RootPlan::Fixes(vec!["connector".into()])), "offered at once, not the whole setup");
}

#[test]
fn several_root_problems_are_one_fix_all_and_user_problems_have_their_own_fix() {
    let mut s = AutoSetup::default();
    s.start(RunKind::Again, false);
    s.checks_done(vec![bad("udev-rule", CheckFix::Root { reboot: false }), bad("setcap-desk", CheckFix::Root { reboot: false }), bad("keepalive", CheckFix::User), ok("headset")]);
    // stage 2 fixed the keep-alive itself; stage 3 took the two root fixes in ONE prompt
    assert_eq!(s.phase, Phase::FixingUser);
    assert_eq!(s.user_fix_done(true), Action::RunRoot(RootPlan::Fixes(vec!["udev-rule".into(), "setcap-desk".into()])));
    s.root_finished(RootOutcome::Ok { reboot: false });
    // still wrong afterwards: the list offers Fix all and one Fix per item
    s.checks_done(vec![bad("udev-rule", CheckFix::Root { reboot: false }), bad("setcap-desk", CheckFix::Root { reboot: false }), bad("keepalive", CheckFix::User), bad("headset", CheckFix::None)]);
    assert_eq!(s.phase, Phase::Problems);
    assert_eq!(s.fix_all(), Action::RunRoot(RootPlan::Fixes(vec!["udev-rule".into(), "setcap-desk".into()])));
    s.root_finished(RootOutcome::Ok { reboot: false });
    s.checks_done(vec![bad("keepalive", CheckFix::User), bad("headset", CheckFix::None)]);
    assert_eq!(s.fix_one("keepalive"), Action::RunUserFix("keepalive".into()));
    assert_eq!(s.user_fix_done(true), Action::RunChecks, "the result is checked again");
    s.checks_done(vec![bad("headset", CheckFix::None)]);
    assert_eq!(s.phase, Phase::Problems);
    assert_eq!(s.fix_one("headset"), Action::None, "only the person can plug the headset in");
    assert_eq!(s.check_again(), Action::RunChecks);
    s.checks_done(all_green());
    assert_eq!(s.phase, Phase::Ready);
}

#[test]
fn cancel_stops_before_the_next_stage() {
    let mut s = fresh();
    s.start(RunKind::Fresh, true);
    s.cancel();
    assert_eq!(s.checks_done(all_green()), Action::None);
    assert_eq!(s.phase, Phase::Cancelled);
    assert!(s.headline().contains("cancelled"));
}

fn item(id: &str, level: Level, detail: &str, fix: Fix) -> Item {
    Item::new(id, crate::checklist::Group::Display, id, level, detail, None, fix)
}

#[test]
fn checks_come_from_the_checklist_with_plain_reasons() {
    let root = |reboot| Fix::Root(RootFix::new("w", vec!["sudo udevadm control --reload".into()], vec![], reboot));
    let none = |_: &Path| false;
    let items = vec![
        item("headset", Level::Ok, "x", Fix::None),
        item("udev-rule", Level::Fail, "not installed", root(false)),
        item("modinfo", Level::Ok, "patched", Fix::None),
        item("merged-head-sync", Level::Fail, "0", Fix::None),
        item("connector", Level::Fail, "DP-1 connected, 7104x3840 missing", Fix::None),
    ];
    let c = checks_from_items(&items, true, &none);
    let get = |id: &str| c.iter().find(|x| x.id == id).unwrap().clone();
    assert_eq!(get("headset").state, CheckState::Ok);
    assert_eq!(get("udev-rule").state, CheckState::Fail);
    assert_eq!(get("udev-rule").fix, CheckFix::Root { reboot: false });
    assert_eq!(get("udev-rule").title, "Permission to use the headset");
    // the patched module is installed but not loaded: it waits for the restart
    assert_eq!(get("merged-head-sync").state, CheckState::WaitsForRestart);
    // no EDID files: a one-click Fix (the root job's EDID mode); with the files in place it waits for the restart
    assert_eq!(get("connector").state, CheckState::Fail);
    assert_eq!(get("connector").fix, CheckFix::Root { reboot: true });
    assert!(get("connector").reason.contains("Press Fix"));
    let c = checks_from_items(&items, true, &|p: &Path| EDID_FILES.iter().any(|f| Path::new(f) == p));
    assert_eq!(c.iter().find(|x| x.id == "connector").unwrap().state, CheckState::WaitsForRestart);
    // not installed (no plan): nothing to run
    let c = checks_from_items(&items, false, &none);
    assert_eq!(c.iter().find(|x| x.id == "connector").unwrap().fix, CheckFix::None);
    // an unpatched module: the modinfo check carries the fix, merged-head-sync is not a second problem
    let items = vec![item("modinfo", Level::Warn, "not patched", root(true)), item("merged-head-sync", Level::Fail, "0", Fix::None)];
    let c = checks_from_items(&items, true, &none);
    assert_eq!(c.iter().find(|x| x.id == "modinfo").unwrap().state, CheckState::Fail);
    assert_eq!(c.iter().find(|x| x.id == "merged-head-sync").unwrap().fix, CheckFix::None);
    // a copy-only root fix is not a button
    let items = vec![item("udev-rule", Level::Fail, "x", Fix::Root(RootFix::new("w", vec!["sudo udevadm control --reload".into()], vec![], false).copy_only("edit first")))];
    assert_eq!(checks_from_items(&items, true, &none)[0].fix, CheckFix::None);
}

#[test]
fn the_edid_mode_passes_the_safety_rules_and_the_restart_button_is_the_only_system_command() {
    let i = Install::of_home(Path::new("/home/u"));
    let line = edid_fix_line(&i);
    assert_eq!(line, "/home/u/.local/share/omarchyvr/data/installer/omarchyvr-root.sh edid /home/u/.local/share/omarchyvr/root-plan.env");
    let f = RootFix::new("w", vec![line], vec![], true);
    let c = f.command().expect("pkexec");
    crate::safety::check(&c).expect("allowed by the safety rules");
    let restart = crate::cmd::Cmd::new("systemctl", &["reboot"]);
    crate::safety::check(&restart).expect("the Restart now button");
    assert!(crate::safety::check(&crate::cmd::Cmd::new("systemctl", &["poweroff"])).is_err());
}

mod choose_headset {
    use super::super::*;
    use crate::headset_choice::{preselect, HeadsetPick};

    const ASLEEP: [(u16, u16); 3] = [(0x34a4, 0x43), (0x34a4, 0x14), (0x2104, 0x561)];

    #[test]
    fn a_sleeping_headset_is_preselected_and_one_click_continues_to_the_ready_card() {
        let mut s = AutoSetup::default();
        s.ask_headset(RunKind::Fresh, preselect(&ASLEEP));
        assert_eq!(s.phase, Phase::ChooseHeadset);
        assert_eq!(s.selected, HeadsetPick::Model(0));
        assert_eq!(s.found.as_deref(), Some("Found on USB: Pimax Dream Air (asleep)"));
        assert!(!s.running(), "nothing runs before the choice");
        assert_eq!(s.headset_continue(), Some(HeadsetPick::Model(0)));
        assert_eq!(s.phase, Phase::GetReady);
        s.headset_back();
        assert_eq!(s.phase, Phase::ChooseHeadset);
    }

    #[test]
    fn nothing_detected_selects_nothing_and_continue_waits_for_a_choice() {
        let mut s = AutoSetup::default();
        s.ask_headset(RunKind::Fresh, preselect(&[]));
        assert_eq!((s.selected, s.found.clone()), (HeadsetPick::Unset, None));
        assert_eq!(s.headset_continue(), None, "Continue does nothing until the person chooses");
        assert_eq!(s.phase, Phase::ChooseHeadset);
        s.select_headset(HeadsetPick::Model(0));
        assert_eq!(s.headset_continue(), Some(HeadsetPick::Model(0)));
    }

    #[test]
    fn other_has_no_ready_card_and_the_caller_starts_the_setup() {
        let mut s = AutoSetup::default();
        s.ask_headset(RunKind::Fresh, preselect(&ASLEEP));
        s.select_headset(HeadsetPick::Other);
        assert_eq!(s.headset_continue(), Some(HeadsetPick::Other));
        assert_eq!(s.phase, Phase::ChooseHeadset);
        assert_eq!(s.start(RunKind::Fresh, true), Action::RunChecks);
        assert_eq!(s.phase, Phase::Looking);
    }
}

// ---- 2026-10-17 retro: the headset's parts, the check after an update ----------------------------------------------

/// Real setup 2026-10-17 12:40: the system part ran (NVIDIA patched, a restart needed) while the headset's control chip
/// was off. The setup must not say "ready" or "restart, all set" while a part is missing; the restart stays offered
/// (`reboot`), so nobody has to edit the stage by hand.
#[test]
fn missing_headset_parts_keep_the_setup_from_ready_and_the_restart_stays_offered() {
    let mut s = fresh();
    s.start(RunKind::Fresh, true);
    let parts_bad = || {
        let mut c = all_green();
        c.retain(|x| x.id != "headset");
        c.push(bad("headset", CheckFix::None));
        c
    };
    assert_eq!(s.checks_done(parts_bad()), Action::RunRoot(RootPlan::Setup));
    s.root_output(APPLY_DONE);
    assert_eq!(s.root_finished(RootOutcome::Ok { reboot: true }), Action::RunChecks);
    assert_eq!(s.checks_done(parts_bad()), Action::None);
    assert_eq!(s.phase, Phase::Problems, "not NeedsRestart, not Ready");
    assert!(s.reboot, "the restart is still offered on the problem screen");
    assert_eq!(s.stage(), None, "the saved stage is not touched");
    assert_eq!(guide_for(&s.problems), Some(false), "the guide for the control chip shows");
    // the headset woke up (the guide re-checks): now all set, one restart left
    assert_eq!(s.check_again(), Action::RunChecks);
    assert_eq!(s.checks_done(all_green()), Action::None);
    assert_eq!(s.phase, Phase::NeedsRestart);
}

#[test]
fn missing_cameras_get_one_restart_without_a_password_then_the_power_cycle_guide() {
    let mut s = AutoSetup::default();
    s.start(RunKind::Fresh, false);
    let mut c = all_green();
    c.push(bad("headset-camera", CheckFix::User));
    c.push(bad("eye-chip", CheckFix::User));
    // stage 2 runs the one restart first, then the eye module's enable
    assert_eq!(s.checks_done(c), Action::RunUserFix("headset-camera".into()));
    assert_eq!(s.user_fix_done(false), Action::RunUserFix("eye-chip".into()));
    assert_eq!(s.user_fix_done(true), Action::RunChecks);
    // the cameras are still off: a problem with the guide (a power cycle), not "ready"
    let mut c = all_green();
    c.push(bad("headset-camera", CheckFix::User));
    c.push(ok("eye-chip"));
    assert_eq!(s.checks_done(c), Action::None);
    assert_eq!(s.phase, Phase::Problems);
    assert_eq!(guide_for(&s.problems), Some(true));
    assert_eq!(s.problems.len(), 1);
}

#[test]
fn the_check_after_an_update_offers_the_lost_realtime_permission_with_one_password() {
    // install.sh relinked monado-service (the kernel dropped cap_sys_nice): the app checks once, by itself
    let mut s = fresh();
    assert_eq!(s.start(RunKind::AfterUpdate, false), Action::RunChecks);
    assert_eq!(s.phase, Phase::Verifying, "only the checks, no root job");
    assert_eq!(s.headline(), "Checking after the update...");
    let mut c = all_green();
    c.retain(|x| x.id != "setcap-desk");
    c.push(bad("setcap-desk", CheckFix::Root { reboot: false }));
    assert_eq!(s.checks_done(c), Action::None);
    assert_eq!(s.phase, Phase::Problems);
    assert_eq!(s.fix_one("setcap-desk"), Action::RunRoot(RootPlan::Fixes(vec!["setcap-desk".into()])));
    assert_eq!(s.root_finished(RootOutcome::Ok { reboot: false }), Action::RunChecks);
    assert_eq!(s.checks_done(all_green()), Action::None);
    assert_eq!(s.phase, Phase::Ready, "no restart for a permission");
    assert_eq!(s.stage(), Some(SetupStage::Done));
    // a stopped password step tries the check again, never the whole system setup
    let mut s = fresh();
    s.start(RunKind::AfterUpdate, false);
    s.checks_done(vec![bad("setcap-desk", CheckFix::Root { reboot: false })]);
    s.fix_one("setcap-desk");
    s.root_finished(RootOutcome::Cancelled);
    assert_eq!(s.retry(), Action::RunChecks);
    assert_eq!((s.kind, s.phase.clone()), (RunKind::AfterUpdate, Phase::Verifying));
    // all green right away: Ready without a word
    let mut s = fresh();
    s.start(RunKind::AfterUpdate, false);
    assert_eq!(s.checks_done(all_green()), Action::None);
    assert_eq!(s.phase, Phase::Ready);
    assert!(RunKind::AfterUpdate.check_only() && RunKind::AfterReboot.check_only() && !RunKind::Again.check_only());
}

#[test]
fn the_check_after_an_update_does_not_pull_the_setup_up_for_a_sleeping_headset() {
    // updated while the headset sleeps (normal when VR is not in use): only what the update can change counts
    let mut c = vec![bad("headset", CheckFix::None), bad("connector", CheckFix::None), bad("headset-camera", CheckFix::User), bad("eye-chip", CheckFix::User), ok("udev"), bad("setcap-desk", CheckFix::Root { reboot: false })];
    update_checks(&mut c);
    let ids: Vec<&str> = c.iter().map(|x| x.id.as_str()).collect();
    assert_eq!(ids, ["udev", "setcap-desk"]);
    let mut s = fresh();
    s.start(RunKind::AfterUpdate, false);
    let mut c = vec![bad("headset", CheckFix::None), ok("setcap-desk")];
    update_checks(&mut c);
    s.checks_done(c);
    assert_eq!(s.phase, Phase::Ready, "nothing to show");
}

#[test]
fn a_sleeping_headset_is_explained_as_asleep_not_unplugged() {
    let mut c = vec![bad("headset", CheckFix::None), ok("udev")];
    // only the bridge and the cameras: asleep
    explain_headset(&mut c, &[(0x34a4, 0x43), (0x34a4, 0x14), (0x2104, 0x561)]);
    assert_eq!(c[0].title, "Headset is awake");
    assert!(c[0].reason.contains("asleep or off") && c[0].reason.contains("power button") && !c[0].reason.contains("Plug it in"), "{}", c[0].reason);
    // nothing on USB: the plain "not seen" stays
    let mut c = vec![bad("headset", CheckFix::None)];
    explain_headset(&mut c, &[]);
    assert_eq!(c[0].title, "headset");
}

#[test]
fn the_display_mode_fix_waits_for_the_headset_instead_of_asking_for_a_password() {
    let asleep = [(0x34a4, 0x43), (0x34a4, 0x14)];
    let mut c = vec![bad("headset", CheckFix::None), bad("connector", CheckFix::Root { reboot: true })];
    explain_headset(&mut c, &asleep);
    assert_eq!(c[1].fix, CheckFix::None, "no password while the display cannot show");
    assert!(c[1].reason.contains("once the headset is awake"), "{}", c[1].reason);
    // awake: the one-password Fix stays
    let mut c = vec![bad("connector", CheckFix::Root { reboot: true })];
    explain_headset(&mut c, &[(0x34a4, 0x44)]);
    assert_eq!(c[0].fix, CheckFix::Root { reboot: true });
    // and the problem screen then has no root fix at all: no password prompt is offered
    let mut s = AutoSetup::default();
    s.start(RunKind::AfterReboot, false);
    let mut c = all_green();
    c.retain(|x| x.id != "connector" && x.id != "headset");
    c.push(bad("headset", CheckFix::None));
    c.push(bad("connector", CheckFix::Root { reboot: true }));
    explain_headset(&mut c, &asleep);
    s.checks_done(c);
    assert_eq!(s.fix_all(), Action::None);
}

#[test]
fn the_headset_parts_are_setup_checks() {
    for id in ["headset", "headset-camera", "eye-chip"] {
        assert!(SETUP_IDS.contains(&id), "{id}");
        assert_ne!(plain(id).0, "Check", "{id} has a plain title");
    }
    assert_eq!(guide_for(&[bad("eye-chip", CheckFix::User)]), None, "the eye module is switched on, no power cycle");
    assert_eq!(guide_for(&[bad("headset-camera", CheckFix::User), bad("headset", CheckFix::None)]), Some(false), "the control chip first");
}

fn demo_core(tag: &str) -> (AppCore, std::path::PathBuf) {
    let d = std::env::temp_dir().join(format!("pimax-autosetup-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let a = AppCore::demo(false, omarchyvr_config::Store::new(d.join("a"), d.join("b")));
    (a, d)
}

/// Real start 2026-10-17 13:10: Work failed with "camera recovery failed: ... only as the very last resort power-cycle
/// the link box". The app now opens the guide itself, with the cameras as what it waits for, and Back returns.
#[test]
fn a_failed_start_or_restart_opens_the_guide_and_back_returns() {
    let (mut a, d) = demo_core("guide");
    a.page = crate::app::Page::Desk;
    let e = format!("omarchyvr: camera recovery failed: the headset's camera (34a4:0014) did not come back 60 s after one restart; {}", omarchyvr_tools::vr::reset::POWER_CYCLE_HINT);
    a.wake_after_failure(&e);
    assert_eq!(a.page, crate::app::Page::Onboarding);
    let w = a.onboarding.wake_only.clone().expect("the guide is open");
    assert!(w.need_camera && w.back == crate::app::Page::Desk && w.reason.contains("power cycle"), "{w:?}");
    assert_eq!(a.wake_target(), Some(true));
    a.close_wake_guide();
    assert_eq!((a.page, a.onboarding.wake_only.clone()), (crate::app::Page::Desk, None));
    // Restart headset with the headset asleep (12:45:45): the guide for the control chip
    a.wake_after_failure("omarchyvr: IF0 hidraw node of 34a4:0044 not found");
    assert_eq!(a.wake_target(), Some(false));
    a.close_wake_guide();
    // any other failure: nothing opens
    a.wake_after_failure("omarchyvr-slam export: no calibration file found");
    assert!(a.onboarding.wake_only.is_none() && a.wake_target().is_none());
    let _ = std::fs::remove_dir_all(d);
}

/// The guide under the problem list keeps its timers while the setup checks again (step A's 90 s must run out for step
/// B to show), and is forgotten when the run is over.
#[test]
fn the_guide_keeps_its_timers_while_the_setup_checks_again() {
    let (mut a, d) = demo_core("guide-timers");
    a.facts.usb = vec![(0x34a4, 0x43), (0x34a4, 0x14)];
    a.facts.at = 1;
    a.onboarding.setup.start(RunKind::AfterReboot, false);
    a.onboarding.setup.checks_done(vec![bad("headset", CheckFix::None), ok("udev")]);
    assert_eq!(a.onboarding.setup.phase, Phase::Problems);
    a.wake_poll();
    assert_eq!(a.onboarding.wake_for, Some(false));
    assert!(matches!(a.onboarding.wake.view(AppCore::wake_now()), crate::wake::Wake::PressPower { .. }));
    let before = a.onboarding.wake;
    // a check runs: the guide stays as it is
    a.onboarding.setup.check_again();
    a.wake_poll();
    assert_eq!(a.onboarding.wake_for, Some(false));
    assert_eq!(a.onboarding.wake, before, "not started over");
    // the run is over: forgotten, the next guide starts fresh
    a.onboarding.setup.checks_done(all_green());
    a.wake_poll();
    assert_eq!(a.onboarding.wake_for, None);
    let _ = std::fs::remove_dir_all(d);
}

/// install.sh left `post-install-check`: the app runs the checks once by itself, after the update, without a click.
#[test]
fn the_check_after_an_update_runs_once_by_itself() {
    let (mut a, d) = demo_core("post");
    a.demo = false;
    a.state.setup = SetupStage::Done;
    a.onboarding.install = crate::install::InstallInfo { loaded: true, post_install: true, ..Default::default() };
    a.page = crate::app::Page::Desk;
    a.post_install_poll();
    assert!(a.onboarding.post_install_started);
    assert_eq!(a.onboarding.setup.kind, RunKind::AfterUpdate);
    match a.onboarding.setup.phase {
        // all green: nothing shown, the request is done
        Phase::Ready => {
            assert!(!a.onboarding.install.post_install);
            assert_eq!(a.page, crate::app::Page::Desk);
        }
        // a problem: the setup screen opens with its Fix
        Phase::Problems => assert_eq!(a.page, crate::app::Page::Onboarding),
        // the checks run in the background
        Phase::Verifying => {}
        ref p => panic!("{p:?}"),
    }
    // never twice in one run of the app, never while the setup itself is not done
    let before = a.onboarding.setup.clone();
    a.post_install_poll();
    assert_eq!(a.onboarding.setup.phase, before.phase);
    let (mut b, d2) = demo_core("post2");
    b.demo = false;
    b.state.setup = SetupStage::Todo;
    b.onboarding.install = crate::install::InstallInfo { loaded: true, post_install: true, ..Default::default() };
    b.post_install_poll();
    assert!(!b.onboarding.post_install_started, "a setup that is not done runs its own checks");
    let _ = std::fs::remove_dir_all(d);
    let _ = std::fs::remove_dir_all(d2);
}

#[test]
fn the_post_install_request_is_read_from_the_install() {
    let d = std::env::temp_dir().join(format!("pimax-postinst-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let i = crate::install::Install { share: d.clone() };
    std::fs::create_dir_all(&d).unwrap();
    assert!(!crate::install::read_info(&i).post_install);
    std::fs::write(i.post_install_check(), "monado=changed\nrealtime_permission=no\n").unwrap();
    assert!(crate::install::read_info(&i).post_install);
    let _ = std::fs::remove_dir_all(d);
}
