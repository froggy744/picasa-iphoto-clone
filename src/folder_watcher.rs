use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

struct WatchHandle {
    stop: Arc<AtomicBool>,
}

#[derive(Default)]
pub struct FolderWatchManager {
    handles: HashMap<i64, WatchHandle>,
}

impl FolderWatchManager {
    pub fn watch(&mut self, folder_id: i64, path: String, updates: Sender<String>) {
        self.unwatch(folder_id);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        thread::spawn(move || watch_folder(path, worker_stop, updates));
        self.handles.insert(folder_id, WatchHandle { stop });
    }

    pub fn unwatch(&mut self, folder_id: i64) {
        if let Some(handle) = self.handles.remove(&folder_id) {
            handle.stop.store(true, Ordering::Release);
        }
    }
}

fn watch_folder(path: String, stop: Arc<AtomicBool>, updates: Sender<String>) {
    let (events, receiver) = mpsc::channel::<notify::Result<Event>>();
    let mut watcher = match RecommendedWatcher::new(events, Config::default()) {
        Ok(watcher) => watcher,
        Err(error) => {
            eprintln!("Could not watch folder {path}: {error}");
            return;
        }
    };
    if let Err(error) = watcher.watch(Path::new(&path), RecursiveMode::Recursive) {
        eprintln!("Could not watch folder {path}: {error}");
        return;
    }

    while !stop.load(Ordering::Acquire) {
        let Ok(event) = receiver.recv_timeout(Duration::from_millis(500)) else {
            continue;
        };
        if !is_relevant(&event) {
            continue;
        }
        // Editors commonly emit several events for one save. Coalesce the burst
        // before asking the existing scanner to refresh the folder.
        while receiver.recv_timeout(Duration::from_millis(250)).is_ok() {}
        if !stop.load(Ordering::Acquire) {
            let _ = updates.send(path.clone());
        }
    }
}

fn is_relevant(event: &notify::Result<Event>) -> bool {
    matches!(
        event,
        Ok(Event {
            kind: EventKind::Create(_)
                | EventKind::Modify(_)
                | EventKind::Remove(_)
                | EventKind::Other,
            ..
        })
    )
}
