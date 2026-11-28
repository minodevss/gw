use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Notify, mpsc};
use tokio::task::JoinSet;

use super::action::Action;
use crate::ops::snapshot::load;

const RELOAD: Duration = Duration::from_secs(1);

pub(crate) fn spawn_loader(
    set: &mut JoinSet<()>,
    home: PathBuf,
    tx: mpsc::Sender<Action>,
    reload: Arc<Notify>,
) {
    set.spawn(async move {
        loop {
            let action = match load(&home).await {
                Ok(snapshot) => Action::Data(Box::new(snapshot)),
                Err(error) => {
                    tracing::warn!(%error, "database read failed");
                    Action::LoadFailed(format!("{error:#}"))
                }
            };
            if tx.send(action).await.is_err() {
                return;
            }
            tokio::select! {
                () = tokio::time::sleep(RELOAD) => {}
                () = reload.notified() => {}
            }
        }
    });
}
