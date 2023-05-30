use crate::v2::block_handler::BlockHandler;
use crate::v2::core::Core;
use crate::v2::network::{Connection, Network, NetworkMessage};
use crate::v2::syncer::{CommitObserver, Syncer, SyncerSignals};
use crate::v2::types::{AuthorityIndex, RoundNumber};
use futures::future::join_all;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Handle;
use tokio::select;
use tokio::sync::{mpsc, Notify};
use tokio::task::JoinHandle;

pub struct NetworkSyncer<H: BlockHandler, C: CommitObserver> {
    inner: Arc<NetworkSyncerInner<H, C>>,
    main_task: JoinHandle<()>,
    stop: mpsc::Receiver<()>,
}

struct NetworkSyncerInner<H: BlockHandler, C: CommitObserver> {
    syncer: RwLock<Syncer<H, Arc<Notify>, C>>,
    notify: Arc<Notify>,
    stop: mpsc::Sender<()>,
}

impl<H: BlockHandler + 'static, C: CommitObserver + 'static> NetworkSyncer<H, C> {
    pub fn start(network: Network, core: Core<H>, commit_period: u64, commit_observer: C) -> Self {
        let handle = Handle::current();
        let notify = Arc::new(Notify::new());
        let mut syncer = Syncer::new(core, commit_period, notify.clone(), commit_observer);
        syncer.force_new_block(0);
        let syncer = RwLock::new(syncer);
        let (stop_sender, stop_receiver) = mpsc::channel(1);
        stop_sender.try_send(()).unwrap(); // occupy the only available permit, so that all other calls to send() will block
        let inner = Arc::new(NetworkSyncerInner {
            notify,
            syncer,
            stop: stop_sender,
        });
        let main_task = handle.spawn(Self::run(network, inner.clone()));
        Self {
            inner,
            main_task,
            stop: stop_receiver,
        }
    }

    pub async fn shutdown(self) -> Syncer<H, Arc<Notify>, C> {
        drop(self.stop);
        // todo - wait for network shutdown as well
        self.main_task.await.ok();
        let Ok(inner) = Arc::try_unwrap(self.inner) else {
            panic!("Shutdown failed - not all resources are freed after main task is compelted");
        };
        inner.syncer.into_inner()
    }

    async fn run(mut network: Network, inner: Arc<NetworkSyncerInner<H, C>>) {
        let mut connections: HashMap<usize, JoinHandle<Option<()>>> = HashMap::new();
        let handle = Handle::current();
        let leader_timeout_task = handle.spawn(Self::leader_timeout_task(inner.clone()));
        while let Some(connection) = inner.recv_or_stopped(network.connection_receiver()).await {
            let peer_id = connection.peer_id;
            if let Some(task) = connections.remove(&peer_id) {
                // wait until previous sync task completes
                task.await.ok();
            }
            let task = handle.spawn(Self::connection_task(connection, inner.clone()));
            connections.insert(peer_id, task);
        }
        join_all(
            connections
                .into_values()
                .chain([leader_timeout_task].into_iter()),
        )
        .await;
    }

    async fn connection_task(
        mut connection: Connection,
        inner: Arc<NetworkSyncerInner<H, C>>,
    ) -> Option<()> {
        let last_seen = inner
            .syncer
            .read()
            .last_seen_by_authority(connection.peer_id as AuthorityIndex);
        connection
            .sender
            .send(NetworkMessage::SubscribeOwnFrom(last_seen))
            .await
            .ok()?;
        let handle = Handle::current();
        let mut subscribe_handler: Option<JoinHandle<Option<()>>> = None;
        while let Some(message) = inner.recv_or_stopped(&mut connection.receiver).await {
            match message {
                NetworkMessage::SubscribeOwnFrom(round) => {
                    eprintln!("sub({round})");
                    if let Some(send_blocks_handler) = subscribe_handler.take() {
                        send_blocks_handler.abort();
                        send_blocks_handler.await.ok();
                    }
                    subscribe_handler = Some(handle.spawn(Self::send_blocks(
                        connection.sender.clone(),
                        inner.clone(),
                        round,
                    )));
                }
                NetworkMessage::Block(block) => {
                    eprintln!("block({block})");
                    inner.syncer.write().add_blocks(vec![block]);
                }
            }
        }
        if let Some(subscribe_handler) = subscribe_handler.take() {
            subscribe_handler.abort();
            subscribe_handler.await.ok();
        }
        None
    }

    async fn send_blocks(
        to: mpsc::Sender<NetworkMessage>,
        inner: Arc<NetworkSyncerInner<H, C>>,
        mut round: RoundNumber,
    ) -> Option<()> {
        loop {
            let notified = inner.notify.notified();
            let blocks = inner.syncer.read().get_own_blocks(round, 10);
            for block in blocks {
                round = block.round();
                to.send(NetworkMessage::Block(block)).await.ok()?;
            }
            notified.await
        }
    }

    async fn leader_timeout_task(inner: Arc<NetworkSyncerInner<H, C>>) -> Option<()> {
        let leader_timeout = Duration::from_secs(1);
        loop {
            let notified = inner.notify.notified();
            let round = inner
                .syncer
                .read()
                .last_own_block()
                .map(|b| b.round())
                .unwrap_or_default();
            select! {
                _sleep = tokio::time::sleep(leader_timeout) => {
                    println!("Timeout");
                    inner.syncer.write().force_new_block(round);
                }
                _notified = notified => {
                    // restart loop
                }
                _stopped = inner.stopped() => {
                    return None;
                }
            }
        }
    }
}

impl<H: BlockHandler + 'static, C: CommitObserver + 'static> NetworkSyncerInner<H, C> {
    // Returns None either if channel is closed or NetworkSyncerInner receives stop signal
    async fn recv_or_stopped<T>(&self, channel: &mut mpsc::Receiver<T>) -> Option<T> {
        select! {
            stopped = self.stop.send(()) => {
                assert!(stopped.is_err());
                None
            }
            data = channel.recv() => {
                data
            }
        }
    }

    async fn stopped(&self) {
        let res = self.stop.send(()).await;
        assert!(res.is_err());
    }
}

impl SyncerSignals for Arc<Notify> {
    fn new_block_ready(&mut self) {
        self.notify_waiters();
    }
}

#[cfg(test)]
mod tests {
    use crate::v2::test_util::{check_commits, network_syncers};
    use std::time::Duration;

    #[tokio::test]
    async fn test_network_sync() {
        let network_syncers = network_syncers().await;
        println!("Started");
        tokio::time::sleep(Duration::from_secs(3)).await;
        println!("Done");
        let mut syncers = vec![];
        for network_syncer in network_syncers {
            let syncer = network_syncer.shutdown().await;
            syncers.push(syncer);
        }

        check_commits(&syncers);
    }
}