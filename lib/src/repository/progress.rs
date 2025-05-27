use std::sync::{Arc, Mutex, MutexGuard};

#[derive(Clone)]
pub struct SharedBuildProgress {
    state: Arc<Mutex<BuildProgress>>,
}

impl SharedBuildProgress {
    pub(super) fn new(state: BuildProgress) -> Self {
        Self { state: Arc::new(Mutex::new(state)) }
    }

    pub fn lock(&self) -> MutexGuard<'_, BuildProgress> {
        self.state.lock().unwrap()
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub enum BuildStage {
    BuildingTaskList,
    BuildingOperations,
    BuildingPackage,
}

#[derive(Debug)]
pub struct BuildProgress {
    /// Per worker progression (not empty and len is stable)
    pub workers: Box<[BuildWorkerProgress]>,

    pub stage: BuildStage,

    /// Current number of bytes processed
    pub processed_bytes: u64,
    /// Number of bytes to process
    pub process_bytes: u64,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub enum BuildTaskStage {
    Init,
}

#[derive(Debug, Clone)]
pub struct BuildWorkerProgress {
    /// Current task name
    pub task_name: Arc<str>,
    /// Current number of bytes processed
    pub processed_bytes: u64,
    /// Number of bytes to process
    pub process_bytes: u64,
}
