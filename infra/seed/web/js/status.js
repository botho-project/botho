/**
 * Botho Seed Node Status - Frontend JavaScript
 *
 * Handles:
 * - RPC calls to node_getStatus and getChainInfo
 * - UI state management and error handling
 * - Auto-refresh with configurable intervals
 */

(function () {
  'use strict';

  // Configuration
  const CONFIG = {
    refreshInterval: 10000, // 10 seconds
  };

  // Constants
  const PICOCREDITS_PER_BTH = 1_000_000_000_000; // 1 BTH = 10^12 picocredits

  // DOM elements
  const elements = {
    // Status card
    statusIndicator: document.getElementById('status-indicator'),
    statusLoading: document.getElementById('status-loading'),
    statusContent: document.getElementById('status-content'),
    statusError: document.getElementById('status-error'),
    retryBtn: document.getElementById('retry-btn'),
    // Metrics
    chainHeight: document.getElementById('chain-height'),
    peerCount: document.getElementById('peer-count'),
    totalTransactions: document.getElementById('total-transactions'),
    uptime: document.getElementById('uptime'),
    // Details
    syncStatus: document.getElementById('sync-status'),
    network: document.getElementById('network'),
    version: document.getElementById('version'),
    scpPeers: document.getElementById('scp-peers'),
    mintingStatus: document.getElementById('minting-status'),
    mempoolSize: document.getElementById('mempool-size'),
    // Chain card
    chainLoading: document.getElementById('chain-loading'),
    chainContent: document.getElementById('chain-content'),
    chainError: document.getElementById('chain-error'),
    tipHash: document.getElementById('tip-hash'),
    difficulty: document.getElementById('difficulty'),
    totalMined: document.getElementById('total-mined'),
    circulatingSupply: document.getElementById('circulating-supply'),
    // Footer
    footerNetwork: document.getElementById('footer-network'),
    lastUpdated: document.getElementById('last-updated'),
  };

  /**
   * Make JSON-RPC call
   */
  async function rpcCall(method, params = {}) {
    const response = await fetch('/rpc', {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
      },
      body: JSON.stringify({
        jsonrpc: '2.0',
        method: method,
        params: params,
        id: Date.now(),
      }),
    });

    if (!response.ok) {
      throw new Error(`HTTP ${response.status}: ${response.statusText}`);
    }

    const data = await response.json();

    if (data.error) {
      throw new Error(data.error.message || 'RPC error');
    }

    return data.result;
  }

  /**
   * Format uptime in seconds to human readable format
   */
  function formatUptime(seconds) {
    if (typeof seconds !== 'number' || seconds < 0) {
      return '—';
    }

    const days = Math.floor(seconds / 86400);
    const hours = Math.floor((seconds % 86400) / 3600);
    const minutes = Math.floor((seconds % 3600) / 60);

    const parts = [];
    if (days > 0) parts.push(`${days}d`);
    if (hours > 0) parts.push(`${hours}h`);
    if (minutes > 0 || parts.length === 0) parts.push(`${minutes}m`);

    return parts.join(' ');
  }

  /**
   * Format large numbers with commas
   */
  function formatNumber(num) {
    if (typeof num !== 'number') return '—';
    return num.toLocaleString();
  }

  /**
   * Format picocredits to BTH with appropriate precision
   */
  function formatBTH(picocredits) {
    if (typeof picocredits !== 'number') return '—';
    const bth = picocredits / PICOCREDITS_PER_BTH;
    if (bth >= 1000000) {
      return (bth / 1000000).toFixed(2) + 'M BTH';
    } else if (bth >= 1000) {
      return (bth / 1000).toFixed(2) + 'K BTH';
    } else if (bth >= 1) {
      return bth.toFixed(2) + ' BTH';
    } else {
      return bth.toFixed(6) + ' BTH';
    }
  }

  /**
   * Truncate hash for display
   */
  function truncateHash(hash) {
    if (!hash || hash.length < 20) return hash || '—';
    return hash.substring(0, 12) + '...' + hash.substring(hash.length - 8);
  }

  /**
   * Format current time for last updated
   */
  function formatLastUpdated() {
    const now = new Date();
    return `Last updated: ${now.toLocaleTimeString()}`;
  }

  /**
   * Load node status
   */
  async function loadNodeStatus() {
    try {
      const status = await rpcCall('node_getStatus', {});

      // Hide loading, show content
      elements.statusLoading.classList.add('hidden');
      elements.statusError.classList.add('hidden');
      elements.statusContent.classList.remove('hidden');

      // Update status indicator
      const isOnline = status.synced !== false;
      if (isOnline) {
        elements.statusIndicator.innerHTML = `
          <span class="w-3 h-3 rounded-full bg-green-500"></span>
          <span class="text-green-400 font-medium">Online</span>
        `;
      } else {
        elements.statusIndicator.innerHTML = `
          <span class="w-3 h-3 rounded-full bg-yellow-500 animate-pulse"></span>
          <span class="text-yellow-400 font-medium">Syncing</span>
        `;
      }

      // Update metrics
      elements.chainHeight.textContent = formatNumber(status.chainHeight || status.height || 0);
      elements.peerCount.textContent = formatNumber(status.peerCount || status.peers || 0);
      elements.totalTransactions.textContent = formatNumber(status.totalTransactions || status.txCount || 0);
      elements.uptime.textContent = formatUptime(status.uptimeSeconds || status.uptime);

      // Update details
      // Sync status
      const synced = status.synced !== false && status.syncStatus !== 'syncing';
      if (synced) {
        elements.syncStatus.innerHTML = `
          <span class="w-2 h-2 rounded-full bg-green-500"></span>
          <span class="text-green-400">Synced</span>
        `;
      } else {
        const syncPercent = status.syncProgress || 0;
        elements.syncStatus.innerHTML = `
          <span class="w-2 h-2 rounded-full bg-yellow-500"></span>
          <span class="text-yellow-400">Syncing ${syncPercent.toFixed(1)}%</span>
        `;
      }

      // Network
      elements.network.textContent = status.network || '—';
      if (elements.footerNetwork) {
        const networkDisplay = status.network ? status.network.replace('botho-', '').replace('mainnet', 'Mainnet').replace('testnet', 'Testnet') : 'Botho Network';
        elements.footerNetwork.textContent = `Botho ${networkDisplay}`;
      }

      // Version
      elements.version.textContent = status.version || status.nodeVersion || '—';

      // SCP peers
      elements.scpPeers.textContent = formatNumber(status.scpPeerCount || status.scpPeers || 0);

      // Minting status
      const minting = status.mintingActive || status.minting;
      const threads = status.mintingThreads || 0;
      if (minting) {
        elements.mintingStatus.innerHTML = `
          <span class="w-2 h-2 rounded-full bg-green-500"></span>
          <span class="text-green-400">Active${threads > 0 ? ` (${threads} threads)` : ''}</span>
        `;
      } else {
        elements.mintingStatus.innerHTML = `
          <span class="w-2 h-2 rounded-full bg-gray-500"></span>
          <span class="text-gray-400">Inactive</span>
        `;
      }

      // Mempool size
      elements.mempoolSize.textContent = formatNumber(status.mempoolSize || 0);

      // Update last updated
      if (elements.lastUpdated) {
        elements.lastUpdated.textContent = formatLastUpdated();
      }

    } catch (error) {
      console.error('Failed to load node status:', error);
      elements.statusLoading.classList.add('hidden');
      elements.statusContent.classList.add('hidden');
      elements.statusError.classList.remove('hidden');

      elements.statusIndicator.innerHTML = `
        <span class="w-3 h-3 rounded-full bg-red-500"></span>
        <span class="text-red-400 font-medium">Offline</span>
      `;
    }
  }

  /**
   * Load chain info
   */
  async function loadChainInfo() {
    try {
      const chainInfo = await rpcCall('getChainInfo', {});

      // Hide loading, show content
      elements.chainLoading.classList.add('hidden');
      elements.chainError.classList.add('hidden');
      elements.chainContent.classList.remove('hidden');

      // Update chain info
      elements.tipHash.textContent = truncateHash(chainInfo.tipHash);
      elements.tipHash.title = chainInfo.tipHash || '';
      elements.difficulty.textContent = formatNumber(chainInfo.difficulty);
      elements.totalMined.textContent = formatBTH(chainInfo.totalMined);
      elements.circulatingSupply.textContent = formatBTH(chainInfo.circulatingSupply);

    } catch (error) {
      console.error('Failed to load chain info:', error);
      elements.chainLoading.classList.add('hidden');
      elements.chainContent.classList.add('hidden');
      elements.chainError.classList.remove('hidden');
    }
  }

  /**
   * Load all data
   */
  async function loadAll() {
    await Promise.all([
      loadNodeStatus(),
      loadChainInfo(),
    ]);
  }

  /**
   * Retry loading data
   */
  function retry() {
    // Reset UI to loading state
    elements.statusLoading.classList.remove('hidden');
    elements.statusContent.classList.add('hidden');
    elements.statusError.classList.add('hidden');
    elements.chainLoading.classList.remove('hidden');
    elements.chainContent.classList.add('hidden');
    elements.chainError.classList.add('hidden');

    elements.statusIndicator.innerHTML = `
      <span class="w-3 h-3 rounded-full bg-gray-500 animate-pulse"></span>
      <span class="text-gray-400">Connecting...</span>
    `;

    loadAll();
  }

  /**
   * Initialize
   */
  function init() {
    // Retry button handler
    if (elements.retryBtn) {
      elements.retryBtn.addEventListener('click', retry);
    }

    // Load data immediately
    loadAll();

    // Set up auto-refresh
    setInterval(loadAll, CONFIG.refreshInterval);
  }

  // Start when DOM is ready
  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', init);
  } else {
    init();
  }
})();
