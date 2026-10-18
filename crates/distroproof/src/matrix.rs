//! Bounded parallel execution of a target matrix.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{mpsc, Semaphore};
use tokio::task::JoinHandle;

use crate::event::{Control, Emitter, EventTx, VmEvent};
use crate::model::{Outcome, TargetResult, VmSpec};
use crate::vm::{Runner, Stop};

/// Handle on a running matrix: control of each target, and waiting.
pub struct Matrix {
    pub controls: HashMap<String, mpsc::UnboundedSender<Control>>,
    handles: Vec<JoinHandle<Option<TargetResult>>>,
}

impl Matrix {
    /// Starts the targets, at most `jobs` at a time. In `interactive` mode, a
    /// finished target can be restarted (`Control::Restart`) as long as its
    /// control channel stays open.
    pub fn start(
        runner: Runner,
        specs: Vec<VmSpec>,
        jobs: usize,
        tx: EventTx,
        interactive: bool,
    ) -> Matrix {
        let slots = Arc::new(Semaphore::new(jobs.max(1)));
        let mut controls = HashMap::new();
        let mut handles = Vec::new();
        for spec in specs {
            let (ctrl_tx, ctrl_rx) = mpsc::unbounded_channel();
            controls.insert(spec.name.clone(), ctrl_tx);
            let ev = Emitter::new(spec.name.clone(), tx.clone());
            ev.emit(VmEvent::Queued);
            handles.push(tokio::spawn(supervise(
                runner.clone(),
                spec,
                ev,
                ctrl_rx,
                slots.clone(),
                interactive,
            )));
        }
        Matrix { controls, handles }
    }

    /// Waits for every target to finish. Results follow the file order.
    pub async fn wait(self) -> Vec<TargetResult> {
        let _controls = self.controls;
        join(self.handles).await
    }

    /// Closes the control channels: running VMs are killed and queued targets
    /// are dropped.
    pub async fn shutdown(self) -> Vec<TargetResult> {
        drop(self.controls);
        join(self.handles).await
    }
}

async fn join(handles: Vec<JoinHandle<Option<TargetResult>>>) -> Vec<TargetResult> {
    let mut results = Vec::new();
    for h in handles {
        if let Ok(Some(r)) = h.await {
            results.push(r);
        }
    }
    results
}

async fn supervise(
    runner: Runner,
    spec: VmSpec,
    ev: Emitter,
    mut ctrl: mpsc::UnboundedReceiver<Control>,
    slots: Arc<Semaphore>,
    interactive: bool,
) -> Option<TargetResult> {
    loop {
        // Wait for a slot; a Kill received meanwhile cancels the target.
        let permit = tokio::select! {
            p = slots.clone().acquire_owned() => p.ok(),
            cmd = ctrl.recv() => match cmd {
                Some(Control::Kill) | None => None,
                _ => continue,
            },
        };
        let (result, stop) = match permit {
            Some(permit) => {
                let (result, stop) = runner.run(&spec, &ev, &mut ctrl).await;
                drop(permit);
                (result, stop)
            }
            None => {
                let mut r = TargetResult::new(&spec);
                r.outcome = Outcome::Killed;
                r.error = Some("interrupted before it started".into());
                ev.emit(VmEvent::Finished(Box::new(r.clone())));
                (r, Some(Stop::Kill))
            }
        };
        let again = stop == Some(Stop::Restart) || (interactive && wait_restart(&mut ctrl).await);
        if !again {
            return Some(result);
        }
        ev.emit(VmEvent::Queued);
    }
}

/// After a target finishes: waits for a restart request or for shutdown.
async fn wait_restart(ctrl: &mut mpsc::UnboundedReceiver<Control>) -> bool {
    loop {
        match ctrl.recv().await {
            Some(Control::Restart) => return true,
            Some(_) => {}
            None => return false,
        }
    }
}
