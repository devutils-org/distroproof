//! Exécution parallèle bornée d'une matrice de cibles.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{mpsc, Semaphore};
use tokio::task::JoinHandle;

use crate::event::{Control, Emitter, EventTx, VmEvent};
use crate::model::{Outcome, TargetResult, VmSpec};
use crate::vm::{Runner, Stop};

/// Poignée sur une matrice en cours : contrôle de chaque cible et attente.
pub struct Matrix {
    pub controls: HashMap<String, mpsc::UnboundedSender<Control>>,
    handles: Vec<JoinHandle<Option<TargetResult>>>,
}

impl Matrix {
    /// Lance les cibles, au plus `jobs` à la fois. En mode `interactive`, une
    /// cible terminée peut être relancée (`Control::Restart`) tant que son
    /// canal de contrôle reste ouvert.
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

    /// Attend la fin de toutes les cibles. Les résultats suivent l'ordre du fichier.
    pub async fn wait(self) -> Vec<TargetResult> {
        let _controls = self.controls;
        join(self.handles).await
    }

    /// Ferme les canaux de contrôle : les VM en cours sont tuées et les
    /// cibles en attente abandonnées.
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
        // Attente d'un slot ; un Kill reçu entre-temps annule la cible.
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
                r.error = Some("interrompue avant son lancement".into());
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

/// Après la fin d'une cible : attend une demande de relance ou la fermeture.
async fn wait_restart(ctrl: &mut mpsc::UnboundedReceiver<Control>) -> bool {
    loop {
        match ctrl.recv().await {
            Some(Control::Restart) => return true,
            Some(_) => {}
            None => return false,
        }
    }
}
