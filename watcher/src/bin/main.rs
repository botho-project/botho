// Copyright (c) 2018-2022 The Botho Foundation
// Copyright (c) 2024 Botho Foundation
#![deny(missing_docs)]
#![doc = include_str!("../../README.md")]

//! A standalone watcher program that can sync data from multiple sources.

use displaydoc::Display;
use bt_watcher::{
    attestation_evidence_collector::AttestationEvidenceCollector,
    config::WatcherConfig,
    watcher::{SyncResult, Watcher},
    watcher_db::create_or_open_rw_watcher_db,
};

use clap::Parser;
use bt_common::logger::{create_app_logger, log, o, Logger};
use bt_util_grpc_tonic::{HealthCheckStatus, HealthService};
use bt_util_uri::ConnectionUri;
use std::{
    io::Error as IOError,
    net::SocketAddr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::{sleep, Builder as ThreadBuilder, JoinHandle},
};
use tonic::transport::Server;

fn main() {
    let _sentry_guard = bt_common::sentry::init();
    let (logger, _global_logger_guard) = create_app_logger(o!());
    bt_common::setup_panic_handler();

    let config = WatcherConfig::parse();
    let sources_config = config.sources_config();

    let watcher_db = create_or_open_rw_watcher_db(
        &config.watcher_db,
        &sources_config.tx_source_urls()[..],
        logger.clone(),
    )
    .expect("Could not create or open watcher db");
    let watcher = Watcher::new(watcher_db.clone(), config.store_block_data, logger.clone())
        .expect("Failed creating watcher");

    let _verification_reports_collector = <AttestationEvidenceCollector>::new(
        watcher_db,
        sources_config.sources().to_vec(),
        config.poll_interval,
        logger.clone(),
    );

    // Start watcher sync thread.
    let mut sync_thread = WatcherSyncThread::start(watcher, config.clone(), logger.clone())
        .expect("Failed starting watcher sync thread.");

    // Start gRPC server using tonic.
    let health_check_callback: Arc<dyn Fn(&str) -> HealthCheckStatus + Sync + Send> =
        Arc::new(move |_| HealthCheckStatus::Serving);
    let health_service = HealthService::new(Some(health_check_callback), logger.clone());

    // Parse the listen address from the URI
    let addr: SocketAddr = config
        .client_listen_uri
        .addr()
        .parse()
        .expect("Could not parse listen address");

    log::info!(logger, "gRPC API listening on {}", addr);

    // Run the tonic server in a separate runtime
    let server_logger = logger.clone();
    let server_handle = std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
        rt.block_on(async {
            if let Err(e) = Server::builder()
                .add_service(health_service.into_service())
                .serve(addr)
                .await
            {
                log::error!(server_logger, "gRPC server error: {}", e);
            }
        });
    });

    // Wait forever for sync thread to exit.
    sync_thread
        .join_handle
        .take()
        .expect("No join handle for watcher sync thread")
        .join()
        .expect("Failed waiting for watcher sync thread");

    // Server thread will be terminated when main exits
    drop(server_handle);
}

/// Possible errors.
#[derive(Debug, Display)]
pub enum Error {
    /// Thread join error
    ThreadJoin,

    /// IO error: {0}
    IO(IOError),
}

impl From<IOError> for Error {
    fn from(src: IOError) -> Self {
        Self::IO(src)
    }
}

/// Thread wrapper for watcher sync.
pub struct WatcherSyncThread {
    /// Join handle used to wait for the thread to terminate.
    join_handle: Option<JoinHandle<()>>,

    /// Stop request trigger, used to signal the thread to stop.
    stop_requested: Arc<AtomicBool>,
}

const MAX_BLOCKS_PER_SYNC_ITERATION: usize = 1000;

impl WatcherSyncThread {
    /// Start the sync thread.
    pub fn start(watcher: Watcher, config: WatcherConfig, logger: Logger) -> Result<Self, Error> {
        let stop_requested = Arc::new(AtomicBool::new(false));
        let thread_stop_requested = stop_requested.clone();

        let join_handle = Some(ThreadBuilder::new().name("WatcherSync".to_string()).spawn(
            move || Self::thread_entrypoint(watcher, config, thread_stop_requested, logger),
        )?);

        Ok(Self {
            join_handle,
            stop_requested,
        })
    }

    /// Stop and join the sync thread.
    pub fn stop(&mut self) -> Result<(), Error> {
        if let Some(join_handle) = self.join_handle.take() {
            self.stop_requested.store(true, Ordering::SeqCst);
            join_handle.join().map_err(|_| Error::ThreadJoin)?;
        }

        Ok(())
    }

    fn thread_entrypoint(
        watcher: Watcher,
        config: WatcherConfig,
        stop_requested: Arc<AtomicBool>,
        logger: Logger,
    ) {
        log::debug!(logger, "Watcher sync thread started");

        loop {
            if stop_requested.load(Ordering::SeqCst) {
                log::debug!(logger, "Watcher sync thread stop requested.");
                break;
            }

            // For now, ignore origin block, as it does not have a signature.
            let sync_result = watcher
                .sync_blocks(
                    1,
                    config.max_block_height,
                    Some(MAX_BLOCKS_PER_SYNC_ITERATION),
                    false,
                )
                .expect("Could not sync signatures");

            watcher.collect_metrics(None);

            // Decide next step before continuing based on sync result
            match sync_result {
                SyncResult::AllBlocksSynced => {
                    log::info!(logger, "Block synchronization done");
                    break;
                }
                SyncResult::BlockSyncError => {
                    log::debug!(logger, "block sync error, sleeping before trying again");
                    sleep(config.poll_interval);
                }
                // sync_blocks exited to check if stop has been requested
                SyncResult::ReachedMaxBlocksPerIteration => {}
            }
        }
    }
}

impl Drop for WatcherSyncThread {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
