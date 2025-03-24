use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use crate::{
    availability_listener::AvailabilityListener, managers::manager::Manager,
    storage::database::SignalDatabase,
};
use anyhow::Result;

pub type RunFlag = Arc<AtomicBool>;

pub trait Persister<T, U>
where
    T: SignalDatabase,
    U: AvailabilityListener + 'static,
{
    type Manager: IntoIterator<Item = Box<dyn Manager>>;

    fn listen(db: T, managers: Self::Manager) -> Self;
    fn get_run_flag(&self) -> &RunFlag;
    async fn persist(&mut self) -> Result<()>;

    async fn stop(&self) {
        self.get_run_flag().store(false, Ordering::Relaxed);
    }
}
