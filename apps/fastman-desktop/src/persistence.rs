use fastman_core::{
    Project, ProjectStore,
    secrets::{SecretChangePlan, SecretStore},
};
use std::{
    collections::VecDeque,
    future::Future,
    pin::Pin,
    sync::{Arc, Condvar, Mutex},
    task::{Context as TaskContext, Poll, Waker},
    thread::{self, JoinHandle},
    time::Duration,
};

struct OneshotState<T> {
    value: Option<T>,
    waker: Option<Waker>,
}

pub(crate) struct OneshotSender<T> {
    state: Arc<Mutex<OneshotState<T>>>,
}

impl<T> OneshotSender<T> {
    pub(crate) fn send(self, value: T) {
        let mut state = self.state.lock().unwrap_or_else(|err| err.into_inner());
        state.value = Some(value);
        if let Some(waker) = state.waker.take() {
            waker.wake();
        }
    }
}

pub(crate) struct OneshotReceiver<T> {
    state: Arc<Mutex<OneshotState<T>>>,
}

impl<T> Future for OneshotReceiver<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Self::Output> {
        let mut state = self.state.lock().unwrap_or_else(|err| err.into_inner());
        if let Some(value) = state.value.take() {
            Poll::Ready(value)
        } else {
            state.waker = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}

pub(crate) fn oneshot<T>() -> (OneshotSender<T>, OneshotReceiver<T>) {
    let state = Arc::new(Mutex::new(OneshotState {
        value: None,
        waker: None,
    }));
    (
        OneshotSender {
            state: Arc::clone(&state),
        },
        OneshotReceiver { state },
    )
}

struct SaveProjectJob {
    store: ProjectStore,
    project: Project,
    revision: u64,
    secrets: Option<(Arc<dyn SecretStore>, SecretChangePlan)>,
    waiters: Vec<OneshotSender<Result<u64, String>>>,
}

enum WorkerJob {
    SaveProject(Box<SaveProjectJob>),
    RunIo(Box<dyn FnOnce() + Send + 'static>),
}

struct QueueState {
    closed: bool,
    busy: bool,
    queue: VecDeque<WorkerJob>,
}

struct WorkerShared {
    state: Mutex<QueueState>,
    work_ready: Condvar,
    idle: Condvar,
}

struct WorkerInner {
    shared: Arc<WorkerShared>,
    #[cfg(test)]
    simulated_delay: Arc<Mutex<Duration>>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl Drop for WorkerInner {
    fn drop(&mut self) {
        {
            let mut state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(|err| err.into_inner());
            state.closed = true;
            self.shared.work_ready.notify_all();
        }
        if let Ok(mut guard) = self.thread.lock()
            && let Some(handle) = guard.take()
        {
            let _ = handle.join();
        }
    }
}

/// Dedicated serialized persistence worker so GPUI handlers never block on JSON
/// serialization, lock acquisition, filesystem writes, or `sync_all()`.
#[derive(Clone)]
pub(crate) struct PersistenceWorker {
    inner: Arc<WorkerInner>,
}

impl PersistenceWorker {
    pub(crate) fn new() -> Self {
        Self::with_simulated_delay(Duration::ZERO)
    }

    fn with_simulated_delay(delay: Duration) -> Self {
        let shared = Arc::new(WorkerShared {
            state: Mutex::new(QueueState {
                closed: false,
                busy: false,
                queue: VecDeque::new(),
            }),
            work_ready: Condvar::new(),
            idle: Condvar::new(),
        });
        #[cfg(test)]
        let simulated_delay = Arc::new(Mutex::new(delay));
        #[cfg(not(test))]
        let _ = delay;

        let worker_shared = Arc::clone(&shared);
        #[cfg(test)]
        let worker_delay = Arc::clone(&simulated_delay);

        let handle = thread::Builder::new()
            .name("fastman-persist".into())
            .spawn(move || {
                loop {
                    let job = {
                        let mut state = worker_shared
                            .state
                            .lock()
                            .unwrap_or_else(|err| err.into_inner());
                        while state.queue.is_empty() && !state.closed {
                            state = worker_shared
                                .work_ready
                                .wait(state)
                                .unwrap_or_else(|err| err.into_inner());
                        }
                        if state.queue.is_empty() && state.closed {
                            state.busy = false;
                            worker_shared.idle.notify_all();
                            break;
                        }
                        state.busy = true;
                        state.queue.pop_front()
                    };

                    if let Some(job) = job {
                        match job {
                            WorkerJob::SaveProject(save) => {
                                #[cfg(test)]
                                {
                                    let delay =
                                        *worker_delay.lock().unwrap_or_else(|err| err.into_inner());
                                    if !delay.is_zero() {
                                        thread::sleep(delay);
                                    }
                                }
                                let outcome = execute_save_with_secrets(
                                    &save.store,
                                    &save.project,
                                    save.secrets.as_ref(),
                                )
                                .map(|()| save.revision);
                                for waiter in save.waiters {
                                    waiter.send(outcome.clone());
                                }
                            }
                            WorkerJob::RunIo(task) => task(),
                        }
                    }

                    let mut state = worker_shared
                        .state
                        .lock()
                        .unwrap_or_else(|err| err.into_inner());
                    state.busy = false;
                    if state.queue.is_empty() {
                        worker_shared.idle.notify_all();
                    }
                }
            })
            .expect("spawn persistence worker thread");

        Self {
            inner: Arc::new(WorkerInner {
                shared,
                #[cfg(test)]
                simulated_delay,
                thread: Mutex::new(Some(handle)),
            }),
        }
    }

    pub(crate) fn submit_save(
        &self,
        store: ProjectStore,
        project: Project,
        revision: u64,
    ) -> OneshotReceiver<Result<u64, String>> {
        let (tx, rx) = oneshot();
        let mut state = self
            .inner
            .shared
            .state
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        if state.closed {
            tx.send(Err("Persistence worker is shut down".into()));
            return rx;
        }
        // Coalesce unstarted plain saves targeting the same project directory and ID so
        // rapid edits only write the newest snapshot while still waking earlier callers.
        if let Some(WorkerJob::SaveProject(queued)) = state.queue.iter_mut().rev().find(|job| {
            matches!(
                job,
                WorkerJob::SaveProject(queued_save)
                    if queued_save.secrets.is_none()
                        && queued_save.store.root() == store.root()
                        && queued_save.project.id == project.id
            )
        }) {
            queued.project = project;
            queued.revision = queued.revision.max(revision);
            queued.waiters.push(tx);
            return rx;
        }

        state
            .queue
            .push_back(WorkerJob::SaveProject(Box::new(SaveProjectJob {
                store,
                project,
                revision,
                secrets: None,
                waiters: vec![tx],
            })));
        self.inner.shared.work_ready.notify_one();
        rx
    }

    pub(crate) fn submit_save_with_secrets(
        &self,
        store: ProjectStore,
        project: Project,
        revision: u64,
        secret_store: Arc<dyn SecretStore>,
        plan: SecretChangePlan,
    ) -> OneshotReceiver<Result<u64, String>> {
        if plan.is_empty() {
            return self.submit_save(store, project, revision);
        }
        let (tx, rx) = oneshot();
        let mut state = self
            .inner
            .shared
            .state
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        if state.closed {
            tx.send(Err("Persistence worker is shut down".into()));
            return rx;
        }
        state
            .queue
            .push_back(WorkerJob::SaveProject(Box::new(SaveProjectJob {
                store,
                project,
                revision,
                secrets: Some((secret_store, plan)),
                waiters: vec![tx],
            })));
        self.inner.shared.work_ready.notify_one();
        rx
    }

    pub(crate) fn submit_io<T, F>(&self, op: F) -> OneshotReceiver<T>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        let (tx, rx) = oneshot();
        let mut state = self
            .inner
            .shared
            .state
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        if state.closed {
            drop(state);
            tx.send(op());
            return rx;
        }
        state.queue.push_back(WorkerJob::RunIo(Box::new(move || {
            tx.send(op());
        })));
        self.inner.shared.work_ready.notify_one();
        rx
    }

    #[cfg(test)]
    pub(crate) fn flush_blocking(&self) {
        let mut state = self
            .inner
            .shared
            .state
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        while state.busy || !state.queue.is_empty() {
            state = self
                .inner
                .shared
                .idle
                .wait(state)
                .unwrap_or_else(|err| err.into_inner());
        }
    }

    #[cfg(test)]
    pub(crate) fn set_simulated_delay(&self, delay: Duration) {
        *self
            .inner
            .simulated_delay
            .lock()
            .unwrap_or_else(|err| err.into_inner()) = delay;
    }
}

fn execute_save_with_secrets(
    store: &ProjectStore,
    project: &Project,
    secrets: Option<&(Arc<dyn SecretStore>, SecretChangePlan)>,
) -> Result<(), String> {
    let mut written_keys: Vec<&str> = Vec::new();
    if let Some((secret_store, plan)) = secrets {
        for (key, value) in &plan.writes {
            if let Err(err) = secret_store.set(key, value) {
                for &written in &written_keys {
                    let _ = secret_store.delete(written);
                }
                return Err(format!("Failed to save secret in credential store: {err}"));
            }
            written_keys.push(key.as_str());
        }
    }

    if let Err(err) = store.save(project) {
        if let Some((secret_store, _)) = secrets {
            for &written in &written_keys {
                let _ = secret_store.delete(written);
            }
        }
        return Err(format!("{err:#}"));
    }

    if let Some((secret_store, plan)) = secrets {
        for old_key in &plan.deletes_after_commit {
            let _ = secret_store.delete(old_key);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fastman_core::model::{ApiRequest, Folder, HttpBody, KeyValue};
    use std::time::Instant;

    fn build_large_project(request_count: usize) -> Project {
        let mut project = Project::new("Benchmark 1k");
        let folder_count = 10;
        let per_folder = request_count / folder_count;
        for f in 0..folder_count {
            let mut folder = Folder::new(format!("Folder {f}"));
            for i in 0..per_folder {
                let mut req = ApiRequest::new(
                    "POST",
                    format!("Request {f}-{i}"),
                    format!("https://api.example.com/v1/items/{f}/{i}?page={i}"),
                );
                req.params.push(KeyValue::new("filter", "active"));
                req.params.push(KeyValue::new("limit", "50"));
                req.headers
                    .push(KeyValue::new("Accept", "application/json"));
                req.headers
                    .push(KeyValue::new("X-Request-Index", format!("{f}-{i}")));
                req.body = HttpBody::Json(format!(
                    r#"{{"folder":{f},"index":{i},"payload":"benchmark-sample-body"}}"#
                ));
                folder.requests.push(req);
            }
            project.collections[0].folders.push(folder);
        }
        project
    }

    fn percentile(sorted: &[Duration], p: usize) -> Duration {
        sorted[(sorted.len() * p).div_ceil(100) - 1]
    }

    #[test]
    fn coalesces_queued_saves_preserving_latest_revision_and_notifying_all_waiters() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProjectStore::at(dir.path());
        let mut project = store.create("Coalesce").unwrap();
        let worker = PersistenceWorker::with_simulated_delay(Duration::from_millis(60));

        // First job keeps the worker busy while two newer revisions queue behind it.
        project.name = "Rev 1".into();
        let _rx1 = worker.submit_save(store.clone(), project.clone(), 1);

        project.name = "Rev 2".into();
        let _rx2 = worker.submit_save(store.clone(), project.clone(), 2);

        project.name = "Rev 3".into();
        let _rx3 = worker.submit_save(store.clone(), project.clone(), 3);

        worker.flush_blocking();
        assert_eq!(store.load(&project.id).unwrap().name, "Rev 3");
    }

    #[test]
    fn secret_change_plan_rolls_back_writes_on_project_save_conflict() {
        use fastman_core::SecretRef;
        use fastman_core::secrets::MemorySecretStore;

        let dir = tempfile::tempdir().unwrap();
        let store = ProjectStore::at(dir.path());
        let other = ProjectStore::at(dir.path());
        let mut project = store.create("Secrets").unwrap();
        let secret_store = Arc::new(MemorySecretStore::new());

        // External conflict before async secret save runs.
        let mut ext = other.load(&project.id).unwrap();
        ext.name = "External".into();
        other.save(&ext).unwrap();

        let worker = PersistenceWorker::new();
        let new_key = format!("fastman:{}:base:api_key", project.id);
        project.secret_variables.insert(
            "api_key".into(),
            SecretRef::Keychain {
                key: new_key.clone(),
            },
        );
        let plan = SecretChangePlan {
            writes: vec![(new_key.clone(), "should_rollback".into())],
            deletes_after_commit: Vec::new(),
        };
        let _rx = worker.submit_save_with_secrets(
            store.clone(),
            project.clone(),
            1,
            secret_store.clone(),
            plan,
        );
        worker.flush_blocking();
        assert_eq!(secret_store.get(&new_key).unwrap(), None);

        // Resolving conflict commits both project and secret cleanly and removes stale key.
        let mut latest = store.load(&project.id).unwrap();
        let old_key = format!("fastman:{}:base:old", project.id);
        secret_store.set(&old_key, "stale").unwrap();
        latest.secret_variables.insert(
            "api_key".into(),
            SecretRef::Keychain {
                key: new_key.clone(),
            },
        );
        let ok_plan = SecretChangePlan {
            writes: vec![(new_key.clone(), "committed_secret".into())],
            deletes_after_commit: vec![old_key.clone()],
        };
        let _rx_ok = worker.submit_save_with_secrets(
            store.clone(),
            latest,
            2,
            secret_store.clone(),
            ok_plan,
        );
        worker.flush_blocking();
        assert_eq!(
            secret_store.get(&new_key).unwrap().as_deref(),
            Some("committed_secret")
        );
        assert_eq!(secret_store.get(&old_key).unwrap(), None);
    }

    #[test]
    #[ignore = "run explicitly in release mode to measure persistence latency"]
    fn persistence_save_latency_benchmark() {
        const LOCAL_SAMPLES: usize = 20;
        const SLOW_SAMPLES: usize = 5;
        let dir = tempfile::tempdir().unwrap();
        let store = ProjectStore::at(dir.path());
        let mut project = build_large_project(1_000);
        store.save(&project).unwrap();

        let mut sync_local = Vec::with_capacity(LOCAL_SAMPLES);
        for i in 0..LOCAL_SAMPLES {
            project.name = format!("Sync {i}");
            let started = Instant::now();
            store.save(&project).unwrap();
            sync_local.push(started.elapsed());
        }
        sync_local.sort_unstable();

        let worker = PersistenceWorker::new();
        let mut async_ui_local = Vec::with_capacity(LOCAL_SAMPLES);
        let mut async_total_local = Vec::with_capacity(LOCAL_SAMPLES);
        for i in 0..LOCAL_SAMPLES {
            project.name = format!("Async {i}");
            let started = Instant::now();
            let _rx = worker.submit_save(store.clone(), project.clone(), i as u64 + 1);
            async_ui_local.push(started.elapsed());
            worker.flush_blocking();
            async_total_local.push(started.elapsed());
        }
        async_ui_local.sort_unstable();
        async_total_local.sort_unstable();

        println!(
            "1,000-request save (local disk): sync p50={:?} p95={:?} max={:?} | async UI return p50={:?} p95={:?} max={:?} | async background commit p50={:?} max={:?}",
            percentile(&sync_local, 50),
            percentile(&sync_local, 95),
            *sync_local.last().unwrap(),
            percentile(&async_ui_local, 50),
            percentile(&async_ui_local, 95),
            *async_ui_local.last().unwrap(),
            percentile(&async_total_local, 50),
            *async_total_local.last().unwrap(),
        );

        let delay = Duration::from_millis(50);
        let mut sync_slow = Vec::with_capacity(SLOW_SAMPLES);
        for i in 0..SLOW_SAMPLES {
            project.name = format!("Sync slow {i}");
            let started = Instant::now();
            thread::sleep(delay);
            store.save(&project).unwrap();
            sync_slow.push(started.elapsed());
        }
        sync_slow.sort_unstable();

        worker.set_simulated_delay(delay);
        let mut async_ui_slow = Vec::with_capacity(SLOW_SAMPLES);
        let mut async_total_slow = Vec::with_capacity(SLOW_SAMPLES);
        for i in 0..SLOW_SAMPLES {
            project.name = format!("Async slow {i}");
            let started = Instant::now();
            let _rx = worker.submit_save(store.clone(), project.clone(), 100 + i as u64);
            async_ui_slow.push(started.elapsed());
            worker.flush_blocking();
            async_total_slow.push(started.elapsed());
        }
        async_ui_slow.sort_unstable();
        async_total_slow.sort_unstable();

        println!(
            "1,000-request save (+50ms simulated storage latency): sync UI freeze p50={:?} max={:?} | async UI return p50={:?} max={:?} | async background commit p50={:?} max={:?}",
            percentile(&sync_slow, 50),
            *sync_slow.last().unwrap(),
            percentile(&async_ui_slow, 50),
            *async_ui_slow.last().unwrap(),
            percentile(&async_total_slow, 50),
            *async_total_slow.last().unwrap(),
        );
    }
}
