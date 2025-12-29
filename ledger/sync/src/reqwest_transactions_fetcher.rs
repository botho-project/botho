// Copyright (c) 2018-2022 The Botho Foundation

//! Implementation of the `TransactionsFetcher` trait that fetches transactions
//! data over http(s) using the `reqwest` library. It can be used, for example,
//! to get transaction data from S3.

use crate::transactions_fetcher_trait::{TransactionFetcherError, TransactionsFetcher};
use displaydoc::Display;
use bth_api::{block_num_to_s3block_path, blockchain, merged_block_num_to_s3block_path};
use bth_blockchain_types::{Block, BlockData, BlockIndex};
use bth_common::{
    logger::{log, Logger},
    lru::LruCache,
    ResponderId,
};
use prost::Message;
use reqwest::Error as ReqwestError;
use std::{
    fs,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};
use url::Url;

/// Default merged blocks bucket sizes. Merged blocks are objects that contain
/// multiple consecutive blocks that have been bundled together in order to
/// reduce the amount of requests needed to get the block data.
/// Notes:
/// - This should match the defaults in `mc-ledger-distribution`.
/// - This must be sorted in descending order.
pub const DEFAULT_MERGED_BLOCKS_BUCKET_SIZES: &[u64] = &[10000, 1000, 100];

/// Maximum number of pre-fetched blocks to keep in cache.
pub const MAX_PREFETCHED_BLOCKS: usize = 10000;

#[derive(Debug, Display)]
pub enum ReqwestTransactionsFetcherError {
    /// Url parse error on {0}: {1}
    UrlParse(String, url::ParseError),

    /// reqwest error on {0}: {1:?}
    ReqwestError(String, ReqwestError),

    /// IO error on {0}: {1:?}
    IO(String, std::io::Error),

    /// Received an invalid block from {0}: {1}
    InvalidBlockReceived(String, String),

    /// No URLs configured
    NoUrlsConfigured,
}

impl From<ReqwestError> for ReqwestTransactionsFetcherError {
    fn from(src: ReqwestError) -> Self {
        ReqwestTransactionsFetcherError::ReqwestError(
            String::from(src.url().map_or("", |v| v.as_str())),
            src,
        )
    }
}

impl TransactionFetcherError for ReqwestTransactionsFetcherError {}

#[derive(Clone)]
pub struct ReqwestTransactionsFetcher {
    /// List of URLs to try and fetch objects from.
    pub source_urls: Vec<Url>,

    /// Client used for HTTP(s) requests.
    client: reqwest::blocking::Client,

    /// Logger.
    logger: Logger,

    /// The most recently used URL index (in `source_urls`).
    source_index_counter: Arc<AtomicU64>,

    /// Cache mapping a `BlockIndex` to `BlockData`, filled by merged blocks
    /// when possible.
    blocks_cache: Arc<Mutex<LruCache<BlockIndex, BlockData>>>,

    /// Merged blocks bucket sizes to attempt fetching.
    merged_blocks_bucket_sizes: Vec<u64>,

    /// Number of successful cache hits when attempting ot get block data.
    /// Used for debugging purposes.
    hits: Arc<AtomicU64>,

    /// Number of cache misses when attempting to get block data.
    /// Used for debugging purposes.
    misses: Arc<AtomicU64>,
}

impl ReqwestTransactionsFetcher {
    pub fn new(
        source_urls: Vec<String>,
        logger: Logger,
    ) -> Result<Self, ReqwestTransactionsFetcherError> {
        Self::new_with_client(source_urls, reqwest::blocking::Client::new(), logger)
    }

    pub fn new_with_client(
        source_urls: Vec<String>,
        client: reqwest::blocking::Client,
        logger: Logger,
    ) -> Result<Self, ReqwestTransactionsFetcherError> {
        let source_urls: Result<Vec<Url>, ReqwestTransactionsFetcherError> = source_urls
            .into_iter()
            // All source_urls must end with a '/'
            .map(|mut url| {
                if !url.ends_with('/') {
                    url.push('/');
                }

                url
            })
            // Parse into a Url object
            .map(|url| {
                Url::parse(&url).map_err(|err| ReqwestTransactionsFetcherError::UrlParse(url, err))
            })
            .collect();

        Ok(Self {
            source_urls: source_urls?,
            client,
            logger,
            source_index_counter: Arc::new(AtomicU64::new(0)),
            blocks_cache: Arc::new(Mutex::new(LruCache::new(MAX_PREFETCHED_BLOCKS))),
            merged_blocks_bucket_sizes: DEFAULT_MERGED_BLOCKS_BUCKET_SIZES.to_vec(),
            hits: Arc::new(AtomicU64::new(0)),
            misses: Arc::new(AtomicU64::new(0)),
        })
    }

    pub fn set_merged_blocks_bucket_sizes(&mut self, bucket_sizes: &[u64]) {
        self.merged_blocks_bucket_sizes = bucket_sizes.to_vec();
    }

    pub fn block_from_url(&self, url: &Url) -> Result<BlockData, ReqwestTransactionsFetcherError> {
        let archive_block: blockchain::ArchiveBlock = self.fetch_protobuf_object(url)?;

        let block_data = BlockData::try_from(&archive_block).map_err(|err| {
            ReqwestTransactionsFetcherError::InvalidBlockReceived(url.to_string(), err.to_string())
        })?;

        Ok(block_data)
    }

    // Fetches multiple blocks (a "merged block") from a given url.
    pub fn blocks_from_url(
        &self,
        url: &Url,
    ) -> Result<Vec<BlockData>, ReqwestTransactionsFetcherError> {
        let archive_blocks: blockchain::ArchiveBlocks = self.fetch_protobuf_object(url)?;

        Vec::<BlockData>::try_from(&archive_blocks).map_err(|err| {
            ReqwestTransactionsFetcherError::InvalidBlockReceived(url.to_string(), err.to_string())
        })
    }

    pub fn get_origin_block_and_transactions(
        &self,
    ) -> Result<BlockData, ReqwestTransactionsFetcherError> {
        self.get_block_data_by_index(0, None)
    }

    fn fetch_protobuf_object<M: Message + Default>(
        &self,
        url: &Url,
    ) -> Result<M, ReqwestTransactionsFetcherError> {
        // Special treatment for file:// to read from a local directory.
        let bytes: Vec<u8> = if url.scheme() == "file" {
            let path = &url[url::Position::BeforeHost..url::Position::AfterPath];
            fs::read(path)
                .map_err(|err| ReqwestTransactionsFetcherError::IO(path.to_string(), err))?
                .to_vec()
        } else {
            let mut response = self.client.get(url.as_str()).send().map_err(|err| {
                ReqwestTransactionsFetcherError::ReqwestError(url.to_string(), err)
            })?;

            let mut bytes = Vec::new();
            response.copy_to(&mut bytes)?;
            bytes
        };

        let obj = M::decode(bytes.as_slice()).map_err(|err| {
            ReqwestTransactionsFetcherError::InvalidBlockReceived(
                url.to_string(),
                format!("protobuf parse failed: {err:?}"),
            )
        })?;

        Ok(obj)
    }

    fn get_cached_block_data(
        &self,
        block_index: BlockIndex,
        expected_block: Option<&Block>,
    ) -> Option<BlockData> {
        // Sanity test.
        if let Some(expected_block) = expected_block {
            assert_eq!(block_index, expected_block.index);
        }

        let mut blocks_cache = self.blocks_cache.lock().expect("mutex poisoned");

        // Note: If this block index is in the cache, we take it out under the
        // assumption that our primary caller, LedgerSyncService, is not
        // going to try and fetch the same block twice if it managed to get
        // a valid block.
        blocks_cache.pop(&block_index).and_then(|block_data| {
            // If we expect a specific Block then compare what the cache had with what we
            // expect.
            if let Some(expected_block) = expected_block {
                if block_data.block() == expected_block {
                    let hits = self.hits.fetch_add(1, Ordering::SeqCst);
                    let misses = self.misses.load(Ordering::SeqCst);
                    log::trace!(
                        self.logger,
                        "Got block #{} from cache (total hits/misses: {}/{})",
                        block_index,
                        hits,
                        misses
                    );
                    Some(block_data)
                } else {
                    log::warn!(
                        self.logger,
                        "Got cached block {:?} but actually requested {:?}! This should not happen",
                        block_data.block(),
                        expected_block
                    );
                    None
                }
            } else if block_data.block().index == block_index {
                Some(block_data)
            } else {
                log::error!(
                    self.logger,
                    "Got cached block #{} but actually requested #{}! This should not happen",
                    block_data.block().index,
                    block_index
                );
                None
            }
        })
    }

    pub fn get_block_data_by_index(
        &self,
        block_index: BlockIndex,
        expected_block: Option<&Block>,
    ) -> Result<BlockData, ReqwestTransactionsFetcherError> {
        // Try and see if we can get this block from our cache.
        if let Some(cached_block_data) = self.get_cached_block_data(block_index, expected_block) {
            return Ok(cached_block_data);
        }

        // Get the source to fetch from.
        let source_index_counter =
            self.source_index_counter.fetch_add(1, Ordering::SeqCst) as usize;
        let source_url = &self.source_urls[source_index_counter % self.source_urls.len()];

        // Try and fetch a merged block if we stand a chance of finding one.
        for bucket in self.merged_blocks_bucket_sizes.iter() {
            if block_index % bucket == 0 {
                log::debug!(
                    self.logger,
                    "Attempting to fetch a merged block for #{} (bucket size {})",
                    block_index,
                    bucket
                );
                let filename = merged_block_num_to_s3block_path(*bucket, block_index)
                    .into_os_string()
                    .into_string()
                    .unwrap();
                let url = source_url
                    .join(&filename)
                    .map_err(|e| ReqwestTransactionsFetcherError::UrlParse(filename.clone(), e))?;

                if let Ok(blocks_data) = self.blocks_from_url(&url) {
                    log::debug!(
                        self.logger,
                        "Got a merged block for #{} (bucket size {}): {} entries @ {:?}",
                        block_index,
                        bucket,
                        blocks_data.len(),
                        std::thread::current().name()
                    );

                    {
                        let mut blocks_cache = self.blocks_cache.lock().expect("mutex poisoned");
                        for block_data in blocks_data.into_iter() {
                            blocks_cache.put(block_data.block().index, block_data);
                        }
                    }

                    // Supposedly we have the block we asked for in the cache now.
                    if let Some(cached_block_data) =
                        self.get_cached_block_data(block_index, expected_block)
                    {
                        return Ok(cached_block_data);
                    }
                }
            }
        }

        // Construct URL for the block we are trying to fetch.
        let filename = block_num_to_s3block_path(block_index)
            .into_os_string()
            .into_string()
            .unwrap();
        let url = source_url
            .join(&filename)
            .map_err(|e| ReqwestTransactionsFetcherError::UrlParse(filename, e))?;

        // Try and get the block.
        log::debug!(
            self.logger,
            "Attempting to fetch block {} from {}",
            block_index,
            url
        );

        let block_data = self.block_from_url(&url)?;

        // If the caller is expecting a specific block, check that we received data for
        // the block they asked for
        if let Some(expected_block) = expected_block {
            if expected_block != block_data.block() {
                return Err(ReqwestTransactionsFetcherError::InvalidBlockReceived(
                    url.to_string(),
                    "block data mismatch".to_string(),
                ));
            }
        }

        let hits = self.hits.load(Ordering::SeqCst);
        let misses = self.misses.fetch_add(1, Ordering::SeqCst);
        log::trace!(
            self.logger,
            "Cache miss while getting block #{} (total hits/misses: {}/{})",
            block_data.block().index,
            hits,
            misses
        );

        // Got what we wanted!
        Ok(block_data)
    }
}

impl TransactionsFetcher for ReqwestTransactionsFetcher {
    type Error = ReqwestTransactionsFetcherError;

    fn get_block_data(
        &self,
        _safe_responder_ids: &[ResponderId],
        block: &Block,
    ) -> Result<BlockData, Self::Error> {
        self.get_block_data_by_index(block.index, Some(block))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bth_common::logger::{test_with_logger, Logger};

    #[test_with_logger]
    fn test_new_normalizes_urls_with_trailing_slash(logger: Logger) {
        let urls = vec![
            "https://example.com/blocks".to_string(),
            "https://example.org/data/".to_string(),
        ];

        let fetcher = ReqwestTransactionsFetcher::new(urls, logger).unwrap();

        // All URLs should end with a trailing slash
        assert!(fetcher.source_urls[0].as_str().ends_with('/'));
        assert!(fetcher.source_urls[1].as_str().ends_with('/'));
        assert_eq!(
            fetcher.source_urls[0].as_str(),
            "https://example.com/blocks/"
        );
        assert_eq!(fetcher.source_urls[1].as_str(), "https://example.org/data/");
    }

    #[test_with_logger]
    fn test_new_with_invalid_url_returns_error(logger: Logger) {
        let urls = vec!["not a valid url".to_string()];

        let result = ReqwestTransactionsFetcher::new(urls, logger);
        assert!(result.is_err());

        match result {
            Err(ReqwestTransactionsFetcherError::UrlParse(url, _)) => {
                assert!(url.contains("not a valid url"));
            }
            _ => panic!("Expected UrlParse error"),
        }
    }

    #[test_with_logger]
    fn test_new_with_empty_urls_succeeds(logger: Logger) {
        let urls: Vec<String> = vec![];

        let fetcher = ReqwestTransactionsFetcher::new(urls, logger).unwrap();
        assert!(fetcher.source_urls.is_empty());
    }

    #[test_with_logger]
    fn test_set_merged_blocks_bucket_sizes(logger: Logger) {
        let urls = vec!["https://example.com/".to_string()];

        let mut fetcher = ReqwestTransactionsFetcher::new(urls, logger).unwrap();

        // Initially uses default bucket sizes
        assert_eq!(
            fetcher.merged_blocks_bucket_sizes,
            DEFAULT_MERGED_BLOCKS_BUCKET_SIZES
        );

        // Set custom bucket sizes
        let custom_sizes = vec![500, 50, 5];
        fetcher.set_merged_blocks_bucket_sizes(&custom_sizes);

        assert_eq!(fetcher.merged_blocks_bucket_sizes, custom_sizes);
    }

    #[test_with_logger]
    fn test_new_with_client(logger: Logger) {
        let urls = vec!["https://example.com/".to_string()];
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap();

        let fetcher = ReqwestTransactionsFetcher::new_with_client(urls, client, logger).unwrap();

        assert_eq!(fetcher.source_urls.len(), 1);
    }

    #[test]
    fn test_error_display_format() {
        let url_parse_err = ReqwestTransactionsFetcherError::UrlParse(
            "bad-url".to_string(),
            url::ParseError::RelativeUrlWithoutBase,
        );
        let display = format!("{}", url_parse_err);
        assert!(display.contains("Url parse error"));
        assert!(display.contains("bad-url"));

        let no_urls_err = ReqwestTransactionsFetcherError::NoUrlsConfigured;
        let display = format!("{}", no_urls_err);
        assert!(display.contains("No URLs configured"));

        let invalid_block_err = ReqwestTransactionsFetcherError::InvalidBlockReceived(
            "https://example.com/block/0".to_string(),
            "block mismatch".to_string(),
        );
        let display = format!("{}", invalid_block_err);
        assert!(display.contains("invalid block"));
        assert!(display.contains("block mismatch"));
    }

    #[test_with_logger]
    fn test_cache_behavior(logger: Logger) {
        let urls = vec!["file:///nonexistent/".to_string()];

        let fetcher = ReqwestTransactionsFetcher::new(urls, logger).unwrap();

        // Cache should start empty
        let cache = fetcher.blocks_cache.lock().unwrap();
        assert_eq!(cache.len(), 0);
        drop(cache);

        // Hits and misses should start at 0
        assert_eq!(fetcher.hits.load(Ordering::SeqCst), 0);
        assert_eq!(fetcher.misses.load(Ordering::SeqCst), 0);
    }

    #[test_with_logger]
    fn test_source_url_rotation(logger: Logger) {
        let urls = vec![
            "https://source1.example.com/".to_string(),
            "https://source2.example.com/".to_string(),
            "https://source3.example.com/".to_string(),
        ];

        let fetcher = ReqwestTransactionsFetcher::new(urls, logger).unwrap();

        // Verify the source_index_counter increments
        let initial = fetcher.source_index_counter.load(Ordering::SeqCst);
        assert_eq!(initial, 0);

        // Simulate multiple accesses - the counter should increment
        // (This would happen in get_block_data_by_index)
        fetcher.source_index_counter.fetch_add(1, Ordering::SeqCst);
        fetcher.source_index_counter.fetch_add(1, Ordering::SeqCst);
        fetcher.source_index_counter.fetch_add(1, Ordering::SeqCst);

        let counter = fetcher.source_index_counter.load(Ordering::SeqCst);
        assert_eq!(counter, 3);

        // Verify round-robin behavior (counter % num_sources)
        let source_idx = counter as usize % fetcher.source_urls.len();
        assert_eq!(source_idx, 0); // 3 % 3 = 0, back to first source
    }

    #[test_with_logger]
    fn test_default_merged_blocks_bucket_sizes_are_sorted_descending(logger: Logger) {
        // Verify the invariant that bucket sizes must be sorted in descending order
        let sizes = DEFAULT_MERGED_BLOCKS_BUCKET_SIZES;

        for i in 0..sizes.len() - 1 {
            assert!(
                sizes[i] > sizes[i + 1],
                "Bucket sizes must be sorted in descending order"
            );
        }
    }

    #[test_with_logger]
    fn test_clone(logger: Logger) {
        let urls = vec!["https://example.com/".to_string()];

        let fetcher = ReqwestTransactionsFetcher::new(urls, logger).unwrap();
        let cloned = fetcher.clone();

        // Cloned instance should share the same Arc references
        assert_eq!(fetcher.source_urls, cloned.source_urls);

        // Incrementing counter in one should affect the other (shared Arc)
        fetcher.source_index_counter.fetch_add(1, Ordering::SeqCst);
        assert_eq!(cloned.source_index_counter.load(Ordering::SeqCst), 1);
    }
}
