use std::any::Any;

pub trait Manager {
    fn as_any(&self) -> &dyn Any;
}

pub fn get<T: 'static>(managers: &[Box<dyn Manager>]) -> Option<&T> {
    managers.iter().find_map(|m| m.as_any().downcast_ref::<T>())
}
