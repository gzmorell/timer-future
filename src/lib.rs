pub mod build_a_timer_future_using_waker {
    use std::{
        future::Future,
        pin::Pin,
        sync::{Arc, Mutex},
        task::{Context, Poll, Waker},
        thread,
        time::Duration,
    };

    #[derive(Clone)]
    pub struct TimerFuture {
        pub shared_state: Arc<Mutex<SharedState>>,
    }

    impl TimerFuture {
        pub fn new(duration: Duration) -> Self {
            let shared_state = Arc::new(Mutex::new(SharedState {
                completed: false,
                waker: None,
            }));
            let new_instance = TimerFuture {
                shared_state: shared_state.clone(),
            };
            let thread_shared_state = shared_state.clone();
            thread::spawn(move || {
                thread::sleep(duration);
                let mut shared_state = thread_shared_state.lock().unwrap();
                shared_state.completed = true;
                if let Some(waker) = shared_state.waker.take() {
                    waker.wake();
                }
            });
            new_instance
        }
        pub fn completed(&self) -> bool {
            self.shared_state.lock().unwrap().completed
        }
    }

    impl Future for TimerFuture {
        type Output = ();

        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            let mut shared_state = self.shared_state.lock().unwrap();
            match shared_state.completed {
                true => Poll::Ready(()),
                false => {
                    shared_state.waker = Some(cx.waker().clone());
                    Poll::Pending
                }
            }
        }
    }

    pub struct SharedState {
        pub completed: bool,
        pub waker: Option<Waker>,
    }

    #[tokio::test]
    async fn test_timer_future() {
        let timer_future = TimerFuture::new(Duration::from_millis(10));
        let tf = timer_future.clone();
        assert!(!tf.completed());
        tokio::time::sleep(Duration::from_millis(100)).await;
        let start = std::time::Instant::now();
        timer_future.await;
        let stop = start.elapsed();
        assert!(tf.completed());
        eprintln!("{:?}", stop);
    }
}

pub mod build_an_executor_to_run_timer_future {
    use crossterm::style::Stylize;
    use futures::{
        future::{BoxFuture, FutureExt},
        task::{waker_ref, ArcWake},
    };
    use std::{
        future::Future,
        sync::{
            mpsc::{sync_channel, Receiver, SyncSender},
            Arc, Mutex,
        },
        task::Context,
    };

    pub struct Executor {
        pub task_receiver: Receiver<Arc<Task>>,
    }

    impl Executor {
        pub fn run(&self) {
            loop {
                eprintln!("{}", "executor loop started".to_string().yellow());

                // Remove the task from the receiver.
                // It it is pending, then the ArcWaker will push it back to the channel
                match self.task_receiver.recv() {
                    Ok(arc_task) => {
                        eprintln!(
                            "{} {}",
                            arc_task.task_name,
                            "running task - start, got task from receiver"
                                .to_string()
                                .yellow(),
                        );
                        let mut future_in_task = arc_task.future.lock().unwrap();
                        match future_in_task.take() {
                            Some(mut future) => {
                                let waker = waker_ref(&arc_task);
                                let context = &mut Context::from_waker(&waker);
                                let poll_result = future.as_mut().poll(context);
                                eprintln!(
                                    "{}",
                                    format!("poll_result: {:?}", poll_result)
                                        .to_string()
                                        .yellow()
                                );
                                match poll_result.is_pending() {
                                    false => {
                                        eprintln!(
                                            "{} {}",
                                            &arc_task.task_name,
                                            "running task is completed".to_string().green()
                                        );
                                    }
                                    true => {
                                        eprintln!(
                                            "{} {}",
                                            &arc_task.task_name,
                                            "running task is still pending, putting back in slot"
                                                .to_string()
                                                .yellow()
                                        );
                                        // We are not done processing the future,
                                        // so put the future back in its task to be run again later
                                        *future_in_task = Some(future);
                                    }
                                }
                            }
                            None => unreachable!(),
                        }
                    }
                    Err(_) => {
                        eprintln!(
                            "{}",
                            "no more tasks to run. task_receiver channel closed"
                                .to_string()
                                .red()
                        );
                        break;
                    }
                }
            }
        }
    }

    pub struct Spawner {
        pub task_sender: SyncSender<Arc<Task>>,
    }

    impl Spawner {
        pub fn spawn(&self, future: impl Future<Output = ()> + 'static + Send, name: &'static str) {
            let pinned_boxed_future = future.boxed();
            let task = Arc::new(Task {
                future: Mutex::new(Some(pinned_boxed_future)),
                task_sender: self.task_sender.clone(),
                task_name: name,
            });
            eprintln!("{}", "task spawned and added to channel".to_string().blue());
            self.task_sender
                .send(task)
                .expect("Failed to send task to task_sender");
        }
    }

    pub struct Task {
        pub future: Mutex<Option<BoxFuture<'static, ()>>>,
        pub task_sender: SyncSender<Arc<Task>>,
        pub task_name: &'static str,
    }

    impl ArcWake for Task {
        fn wake_by_ref(arc_self: &Arc<Self>) {
            let cloned = arc_self.clone();
            arc_self
                .task_sender
                .send(cloned)
                .expect("Failed to send task to task_sender");
            eprintln!(
                "{}",
                "task woken up, added back to channel"
                    .to_string()
                    .underlined()
                    .green()
                    .bold()
            );
        }
    }

    pub fn new_executor_and_spawner() -> (Executor, Spawner) {
        let (task_sender, task_receiver) = sync_channel(100);
        let executor = Executor { task_receiver };
        let spawner = Spawner { task_sender };
        (executor, spawner)
    }

    #[test]
    fn run_spawner_and_executor() {
        use crate::build_a_timer_future_using_waker::TimerFuture;
        let (executor, spawner) = new_executor_and_spawner();
        let timer_future = TimerFuture::new(std::time::Duration::from_millis(300));
        let future2 = TimerFuture::new(std::time::Duration::from_millis(200));
        let results = Arc::new(Mutex::new(Vec::new()));
        let results_clone = results.clone();
        // Spawn the timer_future using the spawner
        spawner.spawn(
            async move {
                results_clone.lock().unwrap().push("Start timer!");
                timer_future.await;
                results_clone.lock().unwrap().push("Stop timer!");
            },
            "task_1",
        );
        spawner.spawn(future2, "task_2");
        // run the executor
        drop(spawner);
        executor.run();
        assert_eq!(
            *results.lock().unwrap(),
            vec!["Start timer!", "Stop timer!"]
        );
    }
}

pub mod local_set {

    #[tokio::test]
    async fn test_local_set() {
        use crossterm::style::Stylize;
        use std::rc::Rc;
        use tokio::{task::LocalSet, time::sleep};
        let non_send_data = Rc::new("!SEND_DATA");
        let local_set = LocalSet::new();

        let non_send_data_clone = non_send_data.clone();
        let async_bloc_1 = async move {
            println!(
                "async_block_1: {}",
                non_send_data_clone.as_ref().yellow().bold()
            );
        };
        let join_handle_1 = local_set.spawn_local(async_bloc_1);
        let _1it = local_set.run_until(join_handle_1).await;
        let non_send_data_clone = non_send_data.clone();
        let async_block2 = async move {
            sleep(std::time::Duration::from_millis(100)).await;
            println!(
                "async_block_2: {}",
                non_send_data_clone.as_ref().green().bold()
            );
        };
        let _it = local_set.run_until(async_block2).await;
        let non_send_data_clone = non_send_data.clone();
        local_set.spawn_local(async move {
            println!(
                "async_block_3: {}",
                non_send_data_clone.as_ref().blue().bold()
            );
        });
        local_set.await;
    }
}

pub mod demo_join_select_spawn {
    use std::time::Duration;
    use tokio::time::sleep;

    pub async fn task_1() {
        sleep(Duration::from_millis(100)).await;
        println!("task_1");
    }

    pub async fn task_2() {
        sleep(Duration::from_millis(100)).await;
        println!("task_2");
    }

    pub async fn task_3() {
        sleep(Duration::from_millis(100)).await;
        println!("task_3");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn test_join() {
        tokio::join!(task_1(), task_2(), task_3());
        println!("All tasks completed");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn test_select() {
        tokio::select! {
            _ = task_1() => println!("task_1 completed"),
            _ = task_2() => println!("task_2 completed"),
            _ = task_3() => println!("task_3 completed"),
        }
        println!("All tasks completed");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn test_spawn() {
        let handle_1 = tokio::spawn(task_1());
        let handle_2 = tokio::spawn(task_2());
        let handle_3 = tokio::spawn(task_3());

        handle_1.await.unwrap();
        handle_2.await.unwrap();
        handle_3.await.unwrap();

        println!("All tasks completed");
    }
}

pub mod async_stream {
    use futures::Stream;
    use std::pin::Pin;

    pub type PinnedInputStream = Pin<Box<dyn Stream<Item = Result<String, String>>>>;

    pub fn get_input_vec() -> Vec<String> {
        vec![
            "hello".to_string(),
            "world".to_string(),
            "foo".to_string(),
            "bar".to_string(),
        ]
    }

    pub fn create_input_stream() -> PinnedInputStream {
        let it = async_stream::stream! {
            for input in get_input_vec() {
                yield Ok(input);
            };
        };
        Box::pin(it)
    }

    #[tokio::test]
    async fn test_async_stream() {
        use futures::StreamExt;
        let mut count = 0;
        let mut it = create_input_stream();
        while let Some(event) = it.next().await {
            eprintln!("event: {:?}", event);
            let lhs = event.unwrap();
            let rhs = get_input_vec()[count].clone();
            assert_eq!(lhs, rhs);
            count += 1;
        }
    }
}
