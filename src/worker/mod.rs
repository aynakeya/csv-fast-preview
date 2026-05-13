mod export;
mod message;
mod row_cache;
mod runtime;
mod snapshot;

use crossbeam_channel::{Receiver, Sender, unbounded};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

pub use message::{Event, Job};
pub use snapshot::CsvSnapshot;

pub struct Worker {
    pub tx: Sender<Job>,
    pub rx: Receiver<Event>,
    cancel_filter: Arc<AtomicBool>,
    cancel_search: Arc<AtomicBool>,
}

impl Worker {
    pub fn spawn() -> Self {
        let (job_tx, job_rx) = unbounded::<Job>();
        let (evt_tx, evt_rx) = unbounded::<Event>();
        let cancel_filter = Arc::new(AtomicBool::new(false));
        let cancel_search = Arc::new(AtomicBool::new(false));

        thread::spawn({
            let cancel_filter = Arc::clone(&cancel_filter);
            let cancel_search = Arc::clone(&cancel_search);
            move || runtime::run_worker(job_rx, evt_tx, cancel_filter, cancel_search)
        });

        Self {
            tx: job_tx,
            rx: evt_rx,
            cancel_filter,
            cancel_search,
        }
    }

    pub fn cancel_filter_now(&self) {
        self.cancel_filter.store(true, Ordering::Relaxed);
    }

    pub fn cancel_search_now(&self) {
        self.cancel_search.store(true, Ordering::Relaxed);
    }
}
