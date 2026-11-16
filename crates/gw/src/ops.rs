pub(crate) mod account;
pub(crate) mod key;
pub(crate) mod log;
pub(crate) mod model;

use std::path::{Path, PathBuf};

use crate::store::{self, Store};

pub(crate) async fn with_store<T: Send + 'static>(
    home: &Path,
    work: impl FnOnce(&mut Store) -> store::Result<T> + Send + 'static,
) -> anyhow::Result<T> {
    let db: PathBuf = crate::home::database(home)?;
    let result = tokio::task::spawn_blocking(move || work(&mut Store::open(&db)?)).await?;
    Ok(result?)
}
