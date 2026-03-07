//! RfbmqDispatcher: TaskDispatch implementation backed by rfbmq file-based queues.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use rfbmq_core::{parse_file, Header, Message, MessageId, Priority as RfbmqPriority, Queue};
use stratum_core::ports::TaskDispatch;
use stratum_core::*;

use crate::error::EngineError;

pub struct RfbmqDispatcher {
    queue: Queue,
    claimed: Mutex<HashMap<String, rfbmq_core::ClaimedMessage>>,
}

impl std::fmt::Debug for RfbmqDispatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RfbmqDispatcher").finish()
    }
}

impl RfbmqDispatcher {
    pub fn init(root: &Path, max_pending: i64) -> Result<Self, EngineError> {
        let queue = Queue::init(root, true, max_pending)
            .map_err(|e| EngineError::Dispatch(e.to_string()))?;
        Ok(Self {
            queue,
            claimed: Mutex::new(HashMap::new()),
        })
    }

    pub fn open(root: &Path) -> Result<Self, EngineError> {
        let queue = Queue::open(root).map_err(|e| EngineError::Dispatch(e.to_string()))?;
        Ok(Self {
            queue,
            claimed: Mutex::new(HashMap::new()),
        })
    }

    pub fn init_or_open(root: &Path, max_pending: i64) -> Result<Self, EngineError> {
        match Self::open(root) {
            Ok(d) => Ok(d),
            Err(_) => Self::init(root, max_pending),
        }
    }
}

fn map_priority(p: TaskPriority) -> RfbmqPriority {
    match p {
        TaskPriority::Critical => RfbmqPriority::Critical,
        TaskPriority::High => RfbmqPriority::High,
        TaskPriority::Normal => RfbmqPriority::Normal,
        TaskPriority::Low => RfbmqPriority::Low,
    }
}

fn parse_depends_on(deps: &[String]) -> Result<Vec<MessageId>, EngineError> {
    deps.iter()
        .map(|s| {
            s.parse::<MessageId>()
                .map_err(|e| EngineError::Dispatch(format!("invalid message id: {e}")))
        })
        .collect()
}

impl RfbmqDispatcher {
    fn finish_task(
        &self,
        task: &ClaimedTask,
        op: impl FnOnce(&Queue, &rfbmq_core::ClaimedMessage) -> Result<(), rfbmq_core::Error>,
    ) -> Result<(), EngineError> {
        let cm = self
            .claimed
            .lock()
            .unwrap()
            .remove(&task.id)
            .ok_or_else(|| EngineError::Dispatch(format!("unknown claimed task: {}", task.id)))?;
        op(&self.queue, &cm).map_err(|e| EngineError::Dispatch(e.to_string()))
    }
}

impl TaskDispatch for RfbmqDispatcher {
    type Error = EngineError;

    fn enqueue(&self, body: &str, opts: DispatchOptions) -> Result<String, Self::Error> {
        let depends_on = parse_depends_on(&opts.depends_on)?;
        let header = Header {
            id: None,
            created_at: String::new(),
            created_by: String::new(),
            priority: map_priority(opts.priority),
            retry_count: 0,
            ttl: opts.ttl,
            tags: opts.tags,
            correlation_id: opts.correlation_id,
            reply_to: opts.reply_to,
            depends_on,
            custom: vec![],
        };
        let mut msg = Message {
            header,
            body: body.to_string(),
        };
        let id = self
            .queue
            .enqueue(&mut msg)
            .map_err(|e| EngineError::Dispatch(e.to_string()))?;
        Ok(id.to_string())
    }

    fn list_ready(&self) -> Result<Vec<String>, Self::Error> {
        let ids = self
            .queue
            .list_ready()
            .map_err(|e| EngineError::Dispatch(e.to_string()))?;
        Ok(ids.into_iter().map(|id| id.to_string()).collect())
    }

    fn dequeue(&self) -> Result<Option<ClaimedTask>, Self::Error> {
        let claimed = self
            .queue
            .dequeue()
            .map_err(|e| EngineError::Dispatch(e.to_string()))?;
        match claimed {
            None => Ok(None),
            Some(cm) => {
                let id = cm.id().to_string();
                let msg =
                    parse_file(cm.path()).map_err(|e| EngineError::Dispatch(e.to_string()))?;
                let reply_to = msg.header.reply_to.clone();
                let body = msg.body;
                self.claimed.lock().unwrap().insert(id.clone(), cm);
                Ok(Some(ClaimedTask { id, body, reply_to }))
            }
        }
    }

    fn complete(&self, task: &ClaimedTask) -> Result<(), Self::Error> {
        self.finish_task(task, Queue::complete)
    }

    fn fail(&self, task: &ClaimedTask) -> Result<(), Self::Error> {
        self.finish_task(task, Queue::fail)
    }

    fn depth(&self) -> Result<i64, Self::Error> {
        self.queue
            .depth()
            .map_err(|e| EngineError::Dispatch(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_dispatcher() -> (RfbmqDispatcher, TempDir) {
        let dir = TempDir::new().unwrap();
        let dispatcher = RfbmqDispatcher::init(dir.path(), 1000).unwrap();
        (dispatcher, dir)
    }

    #[test]
    fn enqueue_dequeue_roundtrip() {
        let (d, _dir) = make_dispatcher();
        d.enqueue("hello world", DispatchOptions::default())
            .unwrap();
        let task = d.dequeue().unwrap().unwrap();
        assert_eq!(task.body, "hello world");
    }

    #[test]
    fn complete_marks_done() {
        let (d, _dir) = make_dispatcher();
        d.enqueue("task1", DispatchOptions::default()).unwrap();
        let task = d.dequeue().unwrap().unwrap();
        d.complete(&task).unwrap();
        assert_eq!(d.depth().unwrap(), 0);
    }

    #[test]
    fn depth_returns_count() {
        let (d, _dir) = make_dispatcher();
        assert_eq!(d.depth().unwrap(), 0);
        d.enqueue("a", DispatchOptions::default()).unwrap();
        d.enqueue("b", DispatchOptions::default()).unwrap();
        assert_eq!(d.depth().unwrap(), 2);
    }

    #[test]
    fn dequeue_empty_returns_none() {
        let (d, _dir) = make_dispatcher();
        assert!(d.dequeue().unwrap().is_none());
    }

    #[test]
    fn init_or_open_creates_then_reopens() {
        let dir = TempDir::new().unwrap();
        let d1 = RfbmqDispatcher::init_or_open(dir.path(), 100).unwrap();
        d1.enqueue("persist", DispatchOptions::default()).unwrap();
        drop(d1);

        let d2 = RfbmqDispatcher::init_or_open(dir.path(), 100).unwrap();
        assert_eq!(d2.depth().unwrap(), 1);
    }
}
