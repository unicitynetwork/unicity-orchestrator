// Tool scheduling and async execution

use anyhow::Result;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tokio::task::JoinHandle;
use uuid::Uuid;

pub struct TaskScheduler {
    task_queue: Arc<RwLock<VecDeque<ScheduledTask>>>,
    workers: Vec<JoinHandle<()>>,
    task_sender: mpsc::UnboundedSender<TaskMessage>,
}

#[derive(Debug, Clone)]
pub struct ScheduledTask {
    pub id: Uuid,
    pub tool_id: String,
    pub priority: TaskPriority,
    pub inputs: HashMap<String, serde_json::Value>,
    pub dependencies: Vec<Uuid>,
    pub scheduled_at: chrono::DateTime<chrono::Utc>,
    pub max_attempts: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum TaskPriority {
    Low = 0,
    Normal = 1,
    High = 2,
    Critical = 3,
}

#[derive(Debug)]
pub enum TaskMessage {
    ScheduleTask(ScheduledTask),
    TaskCompleted(Uuid, Result<TaskResult>),
    CancelTask(Uuid),
}

#[derive(Debug, Clone)]
pub struct TaskResult {
    pub task_id: Uuid,
    pub success: bool,
    pub output: Option<serde_json::Value>,
    pub execution_time: f64,
    pub error: Option<String>,
}

impl TaskScheduler {
    pub fn new(num_workers: usize) -> Self {
        let (task_sender, mut task_receiver) = mpsc::unbounded_channel::<TaskMessage>();
        let task_queue = Arc::new(RwLock::new(VecDeque::new()));

        let mut workers = Vec::new();

        for worker_id in 0..num_workers {
            let queue_clone = task_queue.clone();
            let sender_clone = task_sender.clone();

            let worker = tokio::spawn(async move {
                Self::worker_loop(worker_id, queue_clone, sender_clone).await;
            });

            workers.push(worker);
        }

        Self {
            task_queue,
            workers,
            task_sender,
        }
    }

    pub async fn schedule_task(&self, task: ScheduledTask) -> Result<()> {
        self.task_sender.send(TaskMessage::ScheduleTask(task))?;
        Ok(())
    }

    pub async fn cancel_task(&self, task_id: Uuid) -> Result<()> {
        self.task_sender.send(TaskMessage::CancelTask(task_id))?;
        Ok(())
    }

    async fn worker_loop(
        worker_id: usize,
        queue: Arc<RwLock<VecDeque<ScheduledTask>>>,
        sender: mpsc::UnboundedSender<TaskMessage>,
    ) {
        tracing::info!("Worker {} started", worker_id);

        loop {
            // Get next task
            let task = {
                let mut q = queue.write().await;
                q.pop_front()
            };

            if let Some(task) = task {
                // Check dependencies
                if !Self::dependencies_satisfied(&task) {
                    // Re-queue task for later
                    let mut q = queue.write().await;
                    q.push_back(task);
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                    continue;
                }

                tracing::debug!("Worker {} executing task {}", worker_id, task.id);

                // Execute task
                let result = Self::execute_task(&task).await;

                // Send completion message
                let _ = sender.send(TaskMessage::TaskCompleted(task.id, result));
            } else {
                // No tasks available, wait a bit
                tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            }
        }
    }

    fn dependencies_satisfied(task: &ScheduledTask) -> bool {
        // Simplified - in practice, would track completed tasks
        task.dependencies.is_empty()
    }

    async fn execute_task(task: &ScheduledTask) -> Result<TaskResult> {
        let start = std::time::Instant::now();

        // Simplified execution
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let execution_time = start.elapsed().as_secs_f64();

        Ok(TaskResult {
            task_id: task.id,
            success: true,
            output: Some(serde_json::json!({
                "executed_at": chrono::Utc::now().to_rfc3339(),
                "worker_id": "unknown"
            })),
            execution_time,
            error: None,
        })
    }
}