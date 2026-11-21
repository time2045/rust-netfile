use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Running,
    Done,
    Failed,
}

impl TaskStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskStatus::Running => "running",
            TaskStatus::Done => "done",
            TaskStatus::Failed => "failed",
        }
    }
}

pub struct TaskResult {
    pub path: PathBuf,
    pub filename: String,
    pub size: u64,
}

pub struct Task {
    pub id: String,
    pub kind: String,
    status: Mutex<TaskStatus>,
    message: Mutex<String>,
    done: AtomicU64,
    total: AtomicU64,
    finished_at: Mutex<Option<Instant>>,
    result: Mutex<Option<TaskResult>>,
}

#[derive(Serialize)]
pub struct TaskView {
    pub status: &'static str,
    pub kind: String,
    pub message: String,
    pub done: u64,
    pub total: u64,
}

impl Task {
    fn new(id: String, kind: String, message: String) -> Self {
        Self {
            id,
            kind,
            status: Mutex::new(TaskStatus::Running),
            message: Mutex::new(message),
            done: AtomicU64::new(0),
            total: AtomicU64::new(0),
            finished_at: Mutex::new(None),
            result: Mutex::new(None),
        }
    }

    pub fn add_done(&self, bytes: u64) {
        self.done.fetch_add(bytes, Ordering::Relaxed);
    }

    pub fn set_done(&self, bytes: u64) {
        self.done.store(bytes, Ordering::Relaxed);
    }

    pub fn set_total(&self, bytes: u64) {
        self.total.store(bytes, Ordering::Relaxed);
    }

    pub fn set_message(&self, message: impl Into<String>) {
        if let Ok(mut guard) = self.message.lock() {
            *guard = message.into();
        }
    }

    pub fn finish_done(&self, message: impl Into<String>, result: Option<TaskResult>) {
        if let Ok(mut guard) = self.status.lock() {
            *guard = TaskStatus::Done;
        }
        self.set_message(message);
        if result.is_some() {
            if let Ok(mut guard) = self.result.lock() {
                *guard = result;
            }
        }
        if let Ok(mut guard) = self.finished_at.lock() {
            *guard = Some(Instant::now());
        }
    }

    pub fn finish_failed(&self, message: impl Into<String>) {
        if let Ok(mut guard) = self.status.lock() {
            *guard = TaskStatus::Failed;
        }
        self.set_message(message);
        if let Ok(mut guard) = self.finished_at.lock() {
            *guard = Some(Instant::now());
        }
    }

    pub fn is_finished(&self) -> bool {
        self.status
            .lock()
            .map(|s| *s != TaskStatus::Running)
            .unwrap_or(false)
    }

    pub fn status(&self) -> TaskStatus {
        self.status.lock().map(|s| *s).unwrap_or(TaskStatus::Failed)
    }

    pub fn message(&self) -> String {
        self.message.lock().map(|m| m.clone()).unwrap_or_default()
    }

    pub fn view(&self) -> TaskView {
        TaskView {
            status: self.status().as_str(),
            kind: self.kind.clone(),
            message: self.message(),
            done: self.done.load(Ordering::Relaxed),
            total: self.total.load(Ordering::Relaxed),
        }
    }

    pub fn finished_at(&self) -> Option<Instant> {
        self.finished_at.lock().ok().and_then(|g| *g)
    }

    pub fn result(&self) -> Option<(PathBuf, String, u64)> {
        self.result.lock().ok().and_then(|g| {
            g.as_ref()
                .map(|r| (r.path.clone(), r.filename.clone(), r.size))
        })
    }

    pub fn touch_finished(&self) {
        if let Ok(mut guard) = self.finished_at.lock() {
            *guard = Some(Instant::now());
        }
    }
}

pub struct TaskManager {
    inner: Mutex<HashMap<String, Arc<Task>>>,
}

impl TaskManager {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    pub fn create(&self, kind: &str, message: impl Into<String>) -> Arc<Task> {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let task = Arc::new(Task::new(id.clone(), kind.to_string(), message.into()));
        if let Ok(mut guard) = self.inner.lock() {
            guard.insert(id, task.clone());
        }
        task
    }

    pub fn get(&self, id: &str) -> Option<Arc<Task>> {
        self.inner.lock().ok().and_then(|g| g.get(id).cloned())
    }

    pub fn remove(&self, id: &str) -> Option<Arc<Task>> {
        self.inner.lock().ok().and_then(|mut g| g.remove(id))
    }

    pub fn sweep(&self, ttl: Duration) -> usize {
        let now = Instant::now();
        let expired: Vec<Arc<Task>> = {
            let Ok(mut guard) = self.inner.lock() else {
                return 0;
            };
            let ids: Vec<String> = guard
                .iter()
                .filter(|(_, task)| {
                    task.finished_at()
                        .map(|t| now.duration_since(t) > ttl)
                        .unwrap_or(false)
                })
                .map(|(id, _)| id.clone())
                .collect();
            ids.into_iter().filter_map(|id| guard.remove(&id)).collect()
        };
        let mut removed = 0;
        for task in expired {
            if let Some((path, _name, _size)) = task.result() {
                let _ = std::fs::remove_file(path);
            }
            removed += 1;
        }
        removed
    }

    pub fn remove_finished(&self) -> usize {
        let finished: Vec<Arc<Task>> = {
            let Ok(mut guard) = self.inner.lock() else {
                return 0;
            };
            let ids: Vec<String> = guard
                .iter()
                .filter(|(_, task)| task.is_finished())
                .map(|(id, _)| id.clone())
                .collect();
            ids.into_iter().filter_map(|id| guard.remove(&id)).collect()
        };
        let mut removed = 0;
        for task in finished {
            if let Some((path, _name, _size)) = task.result() {
                let _ = std::fs::remove_file(path);
            }
            removed += 1;
        }
        removed
    }
}

impl Default for TaskManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_lifecycle() {
        let manager = TaskManager::new();
        let task = manager.create("zip", "打包中");
        task.set_total(100);
        task.add_done(40);
        assert_eq!(task.view().done, 40);
        assert_eq!(task.view().status, "running");
        task.finish_done("完成", None);
        assert!(task.is_finished());
        assert_eq!(manager.get(&task.id).unwrap().view().status, "done");
        assert_eq!(
            manager.remove(&task.id).map(|t| t.id.clone()),
            Some(task.id.clone())
        );
    }
}
