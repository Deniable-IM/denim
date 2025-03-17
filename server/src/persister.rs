use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use crate::{
    database::SignalDatabase, managers::manager::Manager,
    message_cache::MessageAvailabilityListener,
};
use anyhow::Result;

pub type RunFlag = Arc<AtomicBool>;

pub trait Persister<T, U>
where
    T: SignalDatabase,
    U: MessageAvailabilityListener + Send + 'static,
{
    type Manager: IntoIterator<Item = Box<dyn Manager>>;

    fn listen(db: T, managers: Self::Manager) -> Self;
    fn get_run_flag(&self) -> &RunFlag;
    async fn persist(&mut self) -> Result<()>;

    async fn stop(&self) {
        self.get_run_flag().store(false, Ordering::Relaxed);
    }
}
