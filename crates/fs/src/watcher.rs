use std::{
    path::{Path, PathBuf},
    sync::mpsc::{SendError, Sender},
};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::error::FsError;

#[derive(Debug)]
pub enum FsEvent {
    Changed(PathBuf),
    Rescan,
}

pub fn watch(
    root: &Path,
    tx: Sender<Result<FsEvent, FsError>>,
) -> Result<RecommendedWatcher, FsError> {
    let mut watcher = notify::recommended_watcher(move |result: notify::Result<Event>| {
        match forward_event(&tx, result) {
            Ok(()) | Err(SendError(_)) => {}
        }
    })?;
    watcher.watch(root, RecursiveMode::Recursive)?;
    Ok(watcher)
}

fn forward_event(
    tx: &Sender<Result<FsEvent, FsError>>,
    result: notify::Result<Event>,
) -> Result<(), SendError<Result<FsEvent, FsError>>> {
    let event = match result {
        Ok(event) => event,
        Err(error) => return tx.send(Err(error.into())),
    };

    if event.need_rescan() {
        return tx.send(Ok(FsEvent::Rescan));
    }
    if matches!(event.kind, EventKind::Access(_)) {
        return Ok(());
    }
    if event.paths.is_empty() {
        return tx.send(Ok(FsEvent::Rescan));
    }
    for path in event.paths {
        tx.send(Ok(FsEvent::Changed(path)))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{AccessKind, Flag, ModifyKind, RenameMode};
    use std::sync::mpsc;

    #[test]
    fn forwards_both_paths_for_rename() {
        let (tx, rx) = mpsc::channel();
        let event = Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::Both)))
            .add_path("old".into())
            .add_path("new".into());

        forward_event(&tx, Ok(event)).unwrap();

        assert!(
            matches!(rx.try_recv().unwrap(), Ok(FsEvent::Changed(path)) if path == Path::new("old"))
        );
        assert!(
            matches!(rx.try_recv().unwrap(), Ok(FsEvent::Changed(path)) if path == Path::new("new"))
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn requests_rescan_when_events_may_be_missing() {
        let (tx, rx) = mpsc::channel();
        let event = Event::new(EventKind::Any).set_flag(Flag::Rescan);

        forward_event(&tx, Ok(event)).unwrap();

        assert!(matches!(rx.try_recv().unwrap(), Ok(FsEvent::Rescan)));
    }

    #[test]
    fn requests_rescan_when_path_is_unknown() {
        let (tx, rx) = mpsc::channel();
        forward_event(&tx, Ok(Event::new(EventKind::Any))).unwrap();
        assert!(matches!(rx.try_recv().unwrap(), Ok(FsEvent::Rescan)));
    }

    #[test]
    fn ignores_access_events() {
        let (tx, rx) = mpsc::channel();
        let event = Event::new(EventKind::Access(AccessKind::Read)).add_path("file".into());
        forward_event(&tx, Ok(event)).unwrap();
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn forwards_watcher_errors() {
        let (tx, rx) = mpsc::channel();
        forward_event(&tx, Err(notify::Error::generic("watch failed"))).unwrap();
        assert!(matches!(rx.try_recv().unwrap(), Err(FsError::Watch(_))));
    }
}
