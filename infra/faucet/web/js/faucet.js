/**
 * Botho Testnet Faucet - Frontend JavaScript
 *
 * Handles:
 * - Drip amount decay calculation based on time since last request
 * - RPC calls to faucet_request, faucet_getStats, and node_getStatus
 * - localStorage tracking for decay timer
 * - UI state management and error handling
 * - Configurable section visibility with URL parameter overrides
 */

(function () {
  'use strict';

  // Configuration with defaults
  const CONFIG = {
    // Section visibility
    showFaucetRequest: true,   // Main request form
    showFaucetStats: true,     // Faucet-specific stats
    showNodeStats: true,       // Host/node stats

    // Refresh intervals (ms)
    faucetStatsInterval: 30000,  // 30s
    nodeStatsInterval: 15000,    // 15s (blocks are ~30s)
  };

  /**
   * Parse URL parameters to override config
   */
  function parseUrlParams() {
    const params = new URLSearchParams(window.location.search);

    // faucet=0/1 controls both request form and faucet stats
    if (params.has('faucet')) {
      const showFaucet = params.get('faucet') !== '0';
      CONFIG.showFaucetRequest = showFaucet;
      CONFIG.showFaucetStats = showFaucet;
    }

    // nodestats=0/1 controls node stats section
    if (params.has('nodestats')) {
      CONFIG.showNodeStats = params.get('nodestats') !== '0';
    }
  }

  // Parse URL params on load
  parseUrlParams();

  // Constants
  const STORAGE_KEY = 'botho_faucet_last_request';
  const PICOCREDITS_PER_BTH = 1_000_000_000_000; // 1 BTH = 10^12 picocredits
  const MAX_DRIP_BTH = 1.0;

  // Decay tiers (hours since last request -> multiplier)
  const DECAY_TIERS = [
    { maxHours: 1, multiplier: 0.1 },
    { maxHours: 6, multiplier: 0.25 },
    { maxHours: 12, multiplier: 0.5 },
    { maxHours: 24, multiplier: 0.75 },
    { maxHours: Infinity, multiplier: 1.0 },
  ];

  // DOM elements
  const elements = {
    // Faucet request
    faucetCard: document.getElementById('faucet-card'),
    addressInput: document.getElementById('address'),
    addressError: document.getElementById('address-error'),
    dripAmount: document.getElementById('drip-amount'),
    dripHint: document.getElementById('drip-hint'),
    requestBtn: document.getElementById('request-btn'),
    resultBanner: document.getElementById('result-banner'),
    resultContent: document.getElementById('result-content'),
    // Faucet stats
    faucetStatusCard: document.getElementById('faucet-status-card'),
    statusLoading: document.getElementById('status-loading'),
    statusContent: document.getElementById('status-content'),
    statusError: document.getElementById('status-error'),
    statusIndicator: document.getElementById('status-indicator'),
    maxAmount: document.getElementById('max-amount'),
    dailyLimit: document.getElementById('daily-limit'),
    dailyDispensed: document.getElementById('daily-dispensed'),
    dailyProgress: document.getElementById('daily-progress'),
    // Node stats
    nodeStatusCard: document.getElementById('node-status-card'),
    nodeStatsLoading: document.getElementById('node-stats-loading'),
    nodeStatsContent: document.getElementById('node-stats-content'),
    nodeStatsError: document.getElementById('node-stats-error'),
    nodeUptime: document.getElementById('node-uptime'),
    nodeHeight: document.getElementById('node-height'),
    nodeScpPeers: document.getElementById('node-scp-peers'),
    nodeTransactions: document.getElementById('node-transactions'),
    nodeSyncStatus: document.getElementById('node-sync-status'),
    nodeMintingStatus: document.getElementById('node-minting-status'),
  };

  // State
  let isRequesting = false;
  let faucetEnabled = true;

  /**
   * Calculate drip amount based on time since last request
   */
  function calculateDripAmount(lastRequestTime) {
    if (!lastRequestTime) {
      return MAX_DRIP_BTH; // First request ever
    }

    const hoursSince = (Date.now() - lastRequestTime) / (1000 * 60 * 60);

    for (const tier of DECAY_TIERS) {
      if (hoursSince < tier.maxHours) {
        return MAX_DRIP_BTH * tier.multiplier;
      }
    }

    return MAX_DRIP_BTH;
  }

  /**
   * Get hours remaining until full drip amount
   */
  function getHoursUntilFull(lastRequestTime) {
    if (!lastRequestTime) return 0;

    const hoursSince = (Date.now() - lastRequestTime) / (1000 * 60 * 60);
    if (hoursSince >= 24) return 0;

    return Math.ceil(24 - hoursSince);
  }

  /**
   * Get last request time from localStorage
   */
  function getLastRequestTime() {
    try {
      const stored = localStorage.getItem(STORAGE_KEY);
      if (stored) {
        return new Date(stored).getTime();
      }
    } catch {
      // localStorage not available
    }
    return null;
  }

  /**
   * Save last request time to localStorage
   */
  function saveLastRequestTime() {
    try {
      localStorage.setItem(STORAGE_KEY, new Date().toISOString());
    } catch {
      // localStorage not available
    }
  }

  /**
   * Format BTH amount for display
   */
  function formatBTH(amount) {
    if (amount >= 1) {
      return amount.toFixed(1) + ' BTH';
    }
    return amount.toFixed(2) + ' BTH';
  }

  /**
   * Format picocredits to BTH for display
   */
  function picocreditsToBTH(picocredits) {
    return picocredits / PICOCREDITS_PER_BTH;
  }

  /**
   * Update drip amount display
   */
  function updateDripDisplay() {
    const lastRequest = getLastRequestTime();
    const dripAmount = calculateDripAmount(lastRequest);
    const hoursUntilFull = getHoursUntilFull(lastRequest);

    elements.dripAmount.textContent = formatBTH(dripAmount);
    elements.requestBtn.textContent = `Request ${formatBTH(dripAmount)}`;

    if (hoursUntilFull > 0 && dripAmount < MAX_DRIP_BTH) {
      elements.dripHint.textContent = `Wait ${hoursUntilFull} more hour${hoursUntilFull > 1 ? 's' : ''} for full ${formatBTH(MAX_DRIP_BTH)}`;
      elements.dripHint.classList.remove('hidden');
    } else {
      elements.dripHint.classList.add('hidden');
    }
  }

  /**
   * Validate address format (view:xxx\nspend:xxx)
   */
  function validateAddress(address) {
    const trimmed = address.trim();
    if (!trimmed) {
      return { valid: false, error: 'Please enter your wallet address' };
    }

    // Check for view: and spend: prefixes
    const hasView = /^view:[a-f0-9]+$/im.test(trimmed);
    const hasSpend = /^spend:[a-f0-9]+$/im.test(trimmed);

    if (!hasView || !hasSpend) {
      return {
        valid: false,
        error: 'Address must include both view: and spend: lines with hex values',
      };
    }

    return { valid: true, address: trimmed };
  }

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
   * Show result banner
   */
  function showResult(type, content) {
    elements.resultBanner.classList.remove('hidden', 'border-green-500/50', 'border-red-500/50', 'border-yellow-500/50');
    elements.resultBanner.classList.add(`border-${type === 'success' ? 'green' : type === 'error' ? 'red' : 'yellow'}-500/50`);
    elements.resultContent.innerHTML = content;
    elements.resultBanner.scrollIntoView({ behavior: 'smooth', block: 'nearest' });
  }

  /**
   * Hide result banner
   */
  function hideResult() {
    elements.resultBanner.classList.add('hidden');
  }

  /**
   * Copy text to clipboard
   */
  async function copyToClipboard(text) {
    try {
      await navigator.clipboard.writeText(text);
      return true;
    } catch {
      return false;
    }
  }

  /**
   * Handle faucet request
   */
  async function handleRequest() {
    if (isRequesting || !faucetEnabled) return;

    hideResult();
    elements.addressError.classList.add('hidden');

    // Validate address
    const validation = validateAddress(elements.addressInput.value);
    if (!validation.valid) {
      elements.addressError.textContent = validation.error;
      elements.addressError.classList.remove('hidden');
      return;
    }

    isRequesting = true;
    elements.requestBtn.disabled = true;
    elements.requestBtn.textContent = 'Requesting...';

    try {
      const dripAmount = calculateDripAmount(getLastRequestTime());
      const picocredits = Math.floor(dripAmount * PICOCREDITS_PER_BTH);

      const result = await rpcCall('faucet_request', {
        address: validation.address,
        amount: picocredits,
      });

      // Save successful request time
      saveLastRequestTime();
      updateDripDisplay();

      // Show success
      const txHash = result.txHash || result.tx_hash || 'unknown';
      showResult(
        'success',
        `
        <div class="flex items-start gap-3">
          <span class="text-green-400 text-xl">&#10003;</span>
          <div class="flex-1">
            <p class="font-semibold text-green-400 mb-2">Success! ${formatBTH(dripAmount)} sent.</p>
            <div class="bg-botho-bg rounded-lg p-3 mb-2">
              <p class="text-sm text-gray-400 mb-1">Transaction Hash:</p>
              <div class="flex items-center gap-2">
                <code class="text-xs font-mono text-gray-300 break-all flex-1">${txHash}</code>
                <button
                  onclick="copyTxHash('${txHash}')"
                  class="shrink-0 px-2 py-1 text-xs bg-botho-border hover:bg-gray-700 rounded transition-colors"
                  title="Copy to clipboard"
                >
                  Copy
                </button>
              </div>
            </div>
            <p class="text-sm text-gray-500">Your BTH should arrive after the next block (~30s)</p>
          </div>
        </div>
      `
      );

      // Refresh stats
      loadFaucetStats();
    } catch (error) {
      let errorMessage = error.message;

      // Parse common error types
      if (errorMessage.includes('rate') || errorMessage.includes('limit')) {
        errorMessage = 'Rate limit exceeded. Please try again later.';
      } else if (errorMessage.includes('disabled')) {
        errorMessage = 'Faucet is currently disabled.';
      } else if (errorMessage.includes('balance') || errorMessage.includes('insufficient')) {
        errorMessage = 'Faucet is temporarily out of funds. Please try again later.';
      }

      showResult(
        'error',
        `
        <div class="flex items-start gap-3">
          <span class="text-red-400 text-xl">&#10007;</span>
          <div>
            <p class="font-semibold text-red-400 mb-1">Request Failed</p>
            <p class="text-sm text-gray-400">${errorMessage}</p>
          </div>
        </div>
      `
      );
    } finally {
      isRequesting = false;
      elements.requestBtn.disabled = false;
      updateDripDisplay();
    }
  }

  /**
   * Load faucet stats
   */
  async function loadFaucetStats() {
    try {
      const stats = await rpcCall('faucet_getStats', {});

      elements.statusLoading.classList.add('hidden');
      elements.statusError.classList.add('hidden');
      elements.statusContent.classList.remove('hidden');

      // Update status indicator
      faucetEnabled = stats.enabled !== false;
      if (faucetEnabled) {
        elements.statusIndicator.innerHTML = `
          <span class="w-2 h-2 rounded-full bg-green-500"></span>
          <span class="text-green-400">Active</span>
        `;
      } else {
        elements.statusIndicator.innerHTML = `
          <span class="w-2 h-2 rounded-full bg-red-500"></span>
          <span class="text-red-400">Disabled</span>
        `;
        elements.requestBtn.disabled = true;
        elements.requestBtn.textContent = 'Faucet Disabled';
      }

      // Update amounts
      const maxAmount = picocreditsToBTH(stats.amount || stats.maxAmount || 10_000_000_000_000);
      const dailyLimit = picocreditsToBTH(stats.dailyLimit || stats.daily_limit || 10_000_000_000_000_000);
      const dailyDispensed = picocreditsToBTH(stats.todayDispensed || stats.today_dispensed || 0);

      elements.maxAmount.textContent = formatBTH(maxAmount);
      elements.dailyLimit.textContent = formatBTH(dailyLimit).replace(' BTH', '') + ' BTH';

      const percentage = dailyLimit > 0 ? (dailyDispensed / dailyLimit) * 100 : 0;
      elements.dailyDispensed.textContent = `${formatBTH(dailyDispensed).replace(' BTH', '')} BTH (${percentage.toFixed(1)}%)`;
      elements.dailyProgress.style.width = `${Math.min(percentage, 100)}%`;
    } catch (error) {
      elements.statusLoading.classList.add('hidden');
      elements.statusContent.classList.add('hidden');
      elements.statusError.classList.remove('hidden');
      elements.statusError.textContent = `Failed to load faucet status: ${error.message}`;
    }
  }

  /**
   * Format uptime in seconds to human readable format
   */
  function formatUptime(seconds) {
    if (typeof seconds !== 'number' || seconds < 0) {
      return 'Unknown';
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
   * Load node stats
   */
  async function loadNodeStats() {
    if (!CONFIG.showNodeStats || !elements.nodeStatusCard) return;

    try {
      const status = await rpcCall('node_getStatus', {});

      elements.nodeStatsLoading.classList.add('hidden');
      elements.nodeStatsError.classList.add('hidden');
      elements.nodeStatsContent.classList.remove('hidden');

      // Update metrics
      if (elements.nodeUptime) {
        elements.nodeUptime.textContent = formatUptime(status.uptime || status.uptimeSeconds);
      }
      if (elements.nodeHeight) {
        elements.nodeHeight.textContent = formatNumber(status.height || status.blockHeight || status.chainHeight);
      }
      if (elements.nodeScpPeers) {
        elements.nodeScpPeers.textContent = formatNumber(status.scpPeers || status.peerCount || status.peers);
      }
      if (elements.nodeTransactions) {
        const txCount = status.transactions || status.transactionCount || status.txCount;
        elements.nodeTransactions.textContent = txCount !== undefined ? formatNumber(txCount) : '—';
      }

      // Sync status
      if (elements.nodeSyncStatus) {
        const synced = status.synced !== false && status.syncStatus !== 'syncing';
        if (synced) {
          elements.nodeSyncStatus.innerHTML = `
            <span class="w-2 h-2 rounded-full bg-green-500"></span>
            <span class="text-green-400">Synced</span>
          `;
        } else {
          const syncPercent = status.syncProgress || status.syncPercent || 0;
          elements.nodeSyncStatus.innerHTML = `
            <span class="w-2 h-2 rounded-full bg-yellow-500"></span>
            <span class="text-yellow-400">Syncing ${syncPercent.toFixed(1)}%</span>
          `;
        }
      }

      // Minting status
      if (elements.nodeMintingStatus) {
        const minting = status.minting || status.mintingActive;
        const threads = status.mintingThreads || status.threads || 0;
        if (minting) {
          elements.nodeMintingStatus.innerHTML = `
            <span class="w-2 h-2 rounded-full bg-green-500"></span>
            <span class="text-green-400">Active${threads > 0 ? ` (${threads} thread${threads > 1 ? 's' : ''})` : ''}</span>
          `;
        } else {
          elements.nodeMintingStatus.innerHTML = `
            <span class="w-2 h-2 rounded-full bg-gray-500"></span>
            <span class="text-gray-400">Inactive</span>
          `;
        }
      }
    } catch (error) {
      elements.nodeStatsLoading.classList.add('hidden');
      elements.nodeStatsContent.classList.add('hidden');
      elements.nodeStatsError.classList.remove('hidden');
      elements.nodeStatsError.innerHTML = `
        <div class="flex items-center justify-between">
          <span>Node status unavailable</span>
          <button onclick="window.retryNodeStats()" class="px-3 py-1 text-xs bg-botho-border hover:bg-gray-700 rounded transition-colors">
            Retry
          </button>
        </div>
      `;
    }
  }

  // Global function for retry button
  window.retryNodeStats = function() {
    if (elements.nodeStatsError) {
      elements.nodeStatsError.classList.add('hidden');
    }
    if (elements.nodeStatsLoading) {
      elements.nodeStatsLoading.classList.remove('hidden');
    }
    loadNodeStats();
  };

  /**
   * Apply section visibility based on config
   */
  function applySectionVisibility() {
    // Faucet request section
    if (elements.faucetCard) {
      elements.faucetCard.classList.toggle('hidden', !CONFIG.showFaucetRequest);
    }

    // Faucet stats section
    if (elements.faucetStatusCard) {
      elements.faucetStatusCard.classList.toggle('hidden', !CONFIG.showFaucetStats);
    }

    // Node stats section
    if (elements.nodeStatusCard) {
      elements.nodeStatusCard.classList.toggle('hidden', !CONFIG.showNodeStats);
    }
  }

  /**
   * Initialize
   */
  function init() {
    // Apply section visibility based on config/URL params
    applySectionVisibility();

    // Faucet request functionality
    if (CONFIG.showFaucetRequest) {
      // Update drip display on load
      updateDripDisplay();

      // Update drip display every minute
      setInterval(updateDripDisplay, 60000);

      // Request button handler
      if (elements.requestBtn) {
        elements.requestBtn.addEventListener('click', handleRequest);
      }

      // Enter key in address field
      if (elements.addressInput) {
        elements.addressInput.addEventListener('keydown', (e) => {
          if (e.key === 'Enter' && !e.shiftKey) {
            e.preventDefault();
            handleRequest();
          }
        });

        // Clear error on input
        elements.addressInput.addEventListener('input', () => {
          elements.addressError.classList.add('hidden');
        });
      }
    }

    // Faucet stats
    if (CONFIG.showFaucetStats) {
      loadFaucetStats();
      setInterval(loadFaucetStats, CONFIG.faucetStatsInterval);
    }

    // Node stats
    if (CONFIG.showNodeStats) {
      loadNodeStats();
      setInterval(loadNodeStats, CONFIG.nodeStatsInterval);
    }
  }

  // Global function for copy button
  window.copyTxHash = async function (hash) {
    const success = await copyToClipboard(hash);
    if (success) {
      const btn = event.target;
      const originalText = btn.textContent;
      btn.textContent = 'Copied!';
      setTimeout(() => {
        btn.textContent = originalText;
      }, 2000);
    }
  };

  // Start when DOM is ready
  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', init);
  } else {
    init();
  }
})();
