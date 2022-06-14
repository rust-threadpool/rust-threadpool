use std::sync::{Arc, Mutex};

pub trait ChannelSender: Send {
    fn send(&self, message: Job) -> Result<(), SendError>;
    fn clone(&self) -> Box<dyn ChannelSender>;
}
pub trait ChannelReceiver: Send + Sync {
    fn receive(&self) -> Result<Job, ReceiveError>;
    fn clone(&self) -> Box<dyn ChannelReceiver>;
}

#[derive(Debug)]
pub enum SendError {
    Full,
    Closed,
}
#[derive(Debug)]
pub enum ReceiveError {
    Empty,
    Closed,
}

/// Specify the kind of message channel used in the `ThreadPool`
#[derive(Clone)]
pub enum ChannelType {
    /// Construct a `ThreadPool` with the channel from the rust standard library.
    ///
    /// This is the default mode
    ///
    /// ```
    /// let pool = threadpool::builder()
    ///     // this is optional as it is the default for now
    ///     .with(threadpool::ChannelType::Std)
    ///     .build();
    ///
    /// let bounded_pool = threadpool::builder()
    ///     .queue_size(4)
    ///     .build();
    /// ```
    Std,
    /// Construct a `ThreadPool` with the unbounded channel from the crossbeam_channel create:
    ///
    /// ```
    /// let pool = threadpool::builder().with(threadpool::ChannelType::Crossbeam).build();
    /// let bounded_pool = threadpool::builder()
    ///     .with(threadpool::ChannelType::Crossbeam)
    ///     .queue_size(4)
    ///     .build();
    /// ```
    Crossbeam,
    // TODO Custom(Box<dyn ChannelSender<T>>, Box<dyn ChannelReceiver<T>>),
}

impl ChannelType {
    pub(crate) fn new(
        &self,
        bound: &Option<usize>,
    ) -> (Box<dyn ChannelSender>, Box<dyn ChannelReceiver>) {
        // TODO ChannelType::Custom(tx, rx) => (tx, rx),
        if let Some(amount) = bound {
            match self {
                ChannelType::Std => {
                    let (tx, rx) = std::sync::mpsc::sync_channel(*amount);

                    (
                        Box::new(StdBoundSender(tx)) as Box<dyn ChannelSender>,
                        Box::new(StdReceiver(Arc::new(Mutex::new(rx)))) as Box<dyn ChannelReceiver>,
                    )
                }
                ChannelType::Crossbeam => {
                    let (tx, rx) = crossbeam_channel::bounded(*amount);

                    (
                        Box::new(CBSender(tx)) as Box<dyn ChannelSender>,
                        Box::new(CBReceiver(rx)) as Box<dyn ChannelReceiver>,
                    )
                }
            }
        } else {
            match self {
                ChannelType::Std => {
                    let (tx, rx) = std::sync::mpsc::channel();

                    (
                        Box::new(StdUnboundSender(tx)) as Box<dyn ChannelSender>,
                        Box::new(StdReceiver(Arc::new(Mutex::new(rx)))) as Box<dyn ChannelReceiver>,
                    )
                }
                ChannelType::Crossbeam => {
                    let (tx, rx) = crossbeam_channel::unbounded();

                    (
                        Box::new(CBSender(tx)) as Box<dyn ChannelSender>,
                        Box::new(CBReceiver(rx)) as Box<dyn ChannelReceiver>,
                    )
                }
            }
        }
    }
}
impl Default for ChannelType {
    fn default() -> Self {
        ChannelType::Std
    }
}

pub trait FnBox {
    fn call_box(self: Box<Self>);
}

impl<F: FnOnce()> FnBox for F {
    fn call_box(self: Box<F>) {
        (*self)()
    }
}

type Thunk<'a> = Box<dyn FnBox + Send + 'a>;
type Job = Thunk<'static>;

/////// Std

pub struct StdUnboundSender(std::sync::mpsc::Sender<Job>);
impl ChannelSender for StdUnboundSender {
    fn send(&self, message: Job) -> Result<(), SendError> {
        use std::sync::mpsc::SendError as SE;
        self.0.send(message).map_err(|_: SE<_>| SendError::Closed)
    }
    fn clone(&self) -> Box<dyn ChannelSender> {
        Box::new(StdUnboundSender(self.0.clone()))
    }
}

pub struct StdBoundSender(std::sync::mpsc::SyncSender<Job>);
impl ChannelSender for StdBoundSender {
    fn send(&self, message: Job) -> Result<(), SendError> {
        use std::sync::mpsc::SendError as SE;
        self.0.send(message).map_err(|_: SE<_>| SendError::Closed)
    }
    fn clone(&self) -> Box<dyn ChannelSender> {
        Box::new(StdBoundSender(self.0.clone()))
    }
}

/// Receives from both kinds of std channel
pub struct StdReceiver(Arc<Mutex<std::sync::mpsc::Receiver<Job>>>);
impl ChannelReceiver for StdReceiver {
    fn receive(&self) -> Result<Job, ReceiveError> {
        let lock = self
            .0
            .lock()
            .expect("Worker thread unable to lock job_receiver");
        use std::sync::mpsc::RecvError as RE;
        lock.recv().map_err(|_: RE| ReceiveError::Closed)
    }
    fn clone(&self) -> Box<dyn ChannelReceiver> {
        Box::new(StdReceiver(self.0.clone()))
    }
}

//////// Crossbeam both

pub struct CBSender(crossbeam_channel::Sender<Job>);
impl ChannelSender for CBSender {
    fn send(&self, message: Job) -> Result<(), SendError> {
        use crossbeam_channel::SendError as SE;
        self.0.send(message).map_err(|_: SE<_>| SendError::Closed)
    }
    fn clone(&self) -> Box<dyn ChannelSender> {
        Box::new(CBSender(self.0.clone()))
    }
}

pub struct CBReceiver(crossbeam_channel::Receiver<Job>);
impl ChannelReceiver for CBReceiver {
    fn receive(&self) -> Result<Job, ReceiveError> {
        use crossbeam_channel::RecvError as RE;
        self.0.recv().map_err(|_: RE| ReceiveError::Closed)
    }
    fn clone(&self) -> Box<dyn ChannelReceiver> {
        Box::new(CBReceiver(self.0.clone()))
    }
}
