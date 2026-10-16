//! After a crash: what the app had started that still runs. Desk/VR sessions run in their own user scope and are
//! meant to outlive the app, so nothing is stopped automatically; the recovery dialog lists them with the same
//! stop actions as the Desk page (`pimax desk stop` / `pimax vr stop --grace 30`, SIGINT by pid after a question).
use crate::facts::{Facts, Live};
use crate::preflight::Action;

/// One thing left running (or left half-done).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Orphan {
    /// What it is.
    pub text: String,
    /// The button, if there is one.
    pub action: Option<Action>,
}

/// What still runs, given what the crashed app was doing (`job` from the report's context).
pub fn find(live: &Live, facts: &Facts, job: Option<&str>) -> Vec<Orphan> {
    let mut v = Vec::new();
    if let Some(s) = &live.service {
        let mode = if live.desk_clients_alive() { "Desk mode" } else { "VR mode" };
        v.push(Orphan {
            text: format!("{mode} is still running (monado-service pid {}). It does not need the app; stop it when you are done.", s.pid),
            action: Some(Action::StopSession),
        });
    } else if live.desk_clients_alive() {
        let names: Vec<&str> = live.session.iter().filter(|p| p.alive && p.name != "monado").map(|p| p.name).collect();
        v.push(Orphan {
            text: format!("Leftovers of a session run without Monado: {}.", names.join(", ")),
            action: Some(Action::StopSession),
        });
    }
    for pid in &live.foreign_monado {
        v.push(Orphan {
            text: format!("monado-service pid {pid} runs outside the launcher session."),
            action: Some(Action::StopForeignMonado),
        });
    }
    for (pid, comm) in &facts.stray_clients {
        v.push(Orphan {
            text: format!("{comm} (pid {pid}) runs outside the session (an OpenXR client from a tool)."),
            action: Some(Action::InterruptPid { pid: *pid, comm: comm.clone() }),
        });
    }
    if let Some(j) = job.filter(|j| !j.is_empty()) {
        v.push(Orphan {
            text: format!("This was running when the app stopped: {j}. Its output went on into the job log; the Desk page shows the state now."),
            action: None,
        });
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::{ServiceInfo, SessionProc};

    #[test]
    fn lists_sessions_foreign_monado_strays_and_the_job() {
        let mut live = Live::default();
        let facts = Facts::default();
        assert!(find(&live, &facts, None).is_empty());
        live.session.push(SessionProc { name: "wayvr", pid: Some(9), alive: true, age: None });
        let o = find(&live, &facts, None);
        assert_eq!(o.len(), 1);
        assert!(o[0].text.contains("wayvr") && o[0].action == Some(Action::StopSession));
        live.service = Some(ServiceInfo {
            pid: 42,
            age: None,
            build: None,
            hz: None,
            eye: false,
            sixdof: false,
            ctrl_track: false,
            broker: false,
            ipd: None,
            anchor: false,
        });
        live.foreign_monado = vec![77];
        let facts = Facts {
            stray_clients: vec![(5, "pimax-lenstest".into())],
            ..Default::default()
        };
        let o = find(&live, &facts, Some("Start desk mode (--eye)"));
        assert!(o[0].text.starts_with("Desk mode is still running (monado-service pid 42)"));
        assert_eq!(o[1].action, Some(Action::StopForeignMonado));
        assert_eq!(o[2].action, Some(Action::InterruptPid { pid: 5, comm: "pimax-lenstest".into() }));
        assert!(o[3].text.contains("Start desk mode") && o[3].action.is_none());
    }
}
