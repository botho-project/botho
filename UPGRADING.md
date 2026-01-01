# Botho Network Upgrade Guide

This document describes how to upgrade Botho nodes during protocol changes. Following these procedures ensures network stability and prevents unintended forks.

## Table of Contents

- [Upgrade Types](#upgrade-types)
- [Version Compatibility](#version-compatibility)
- [Upgrade Process](#upgrade-process)
- [Rollback Procedures](#rollback-procedures)
- [Emergency Response](#emergency-response)
- [Upgrade History](#upgrade-history)

## Upgrade Types

### Soft Fork (Minor Version)

A soft fork introduces backward-compatible changes. Older nodes can still process blocks and transactions from newer nodes.

**Criteria:**
- New features that are optional or additive
- Changes that don't affect consensus rules for existing functionality
- Performance improvements that don't change block/transaction format

**Example:** Adding a new memo type that older nodes can safely ignore.

**Timeline:**
1. Announcement: 30 days before activation
2. Recommended upgrade window: 14 days
3. Grace period: 7 days (old nodes still work but may miss features)

### Hard Fork (Major Version)

A hard fork introduces breaking changes. All nodes must upgrade before the activation point.

**Criteria:**
- Changes to consensus rules
- Breaking changes to block or transaction format
- Changes that affect validation logic
- New required features

**Example:** Adding cluster tags for progressive fees (v5).

**Timeline:**
1. Announcement: 60 days before activation
2. Mandatory upgrade deadline: 14 days before activation
3. Activation: At specified block height or timestamp
4. No grace period (non-upgraded nodes will fork)

## Version Compatibility

### Protocol Version Format

```
<major>.<minor>.<patch>
```

- **Major**: Breaking changes (hard fork required)
- **Minor**: New features (soft fork, backward compatible)
- **Patch**: Bug fixes (no consensus impact)

### Agent Version String

Nodes identify themselves using the agent version string:

```
botho/<protocol_version>/<block_version>
```

Example: `botho/1.0.0/5` indicates protocol version 1.0.0 with block version 5 support.

### Backward Compatibility Guarantee

Botho maintains **N-1 version support**:
- Current version (N) is fully supported
- Previous version (N-1) has limited support during transition
- Versions older than N-1 may be rejected

### Checking Your Version

```bash
# Check node version
botho --version

# Check connected peer versions
botho rpc peers --show-versions
```

## Upgrade Process

### Before Upgrading

1. **Check current version and requirements:**
   ```bash
   botho --version
   curl -s https://api.github.com/repos/botho-project/botho/releases/latest | jq -r '.tag_name'
   ```

2. **Review release notes:**
   - Check for breaking changes
   - Note any configuration changes
   - Identify data migration requirements

3. **Backup critical data:**
   ```bash
   # Backup wallet data
   cp -r ~/.botho/wallet ~/.botho/wallet.backup

   # Backup ledger (if validator)
   cp -r ~/.botho/ledger ~/.botho/ledger.backup

   # Backup configuration
   cp ~/.botho/config.toml ~/.botho/config.toml.backup
   ```

4. **Check peer connectivity:**
   ```bash
   botho rpc peers
   ```

### During Upgrade

1. **Stop the node gracefully:**
   ```bash
   botho stop
   # Or if using systemd:
   sudo systemctl stop botho
   ```

2. **Verify node is stopped:**
   ```bash
   pgrep -f botho
   # Should return nothing
   ```

3. **Install new version:**
   ```bash
   # From source
   git fetch origin
   git checkout v<new_version>
   cargo build --release

   # Or download binary
   curl -LO https://github.com/botho-project/botho/releases/download/v<new_version>/botho-<platform>
   chmod +x botho-<platform>
   mv botho-<platform> /usr/local/bin/botho
   ```

4. **Run any required migrations:**
   ```bash
   botho migrate --from-version <old> --to-version <new>
   ```

5. **Start the node:**
   ```bash
   botho start
   # Or if using systemd:
   sudo systemctl start botho
   ```

6. **Verify upgrade:**
   ```bash
   botho --version
   botho rpc status
   botho rpc peers
   ```

### Post-Upgrade Verification

1. **Check sync status:**
   ```bash
   botho rpc sync-status
   ```

2. **Verify peer connections:**
   ```bash
   botho rpc peers --show-versions
   ```

3. **Check for version warnings:**
   ```bash
   grep "version warning" ~/.botho/logs/botho.log
   ```

4. **Monitor for errors:**
   ```bash
   tail -f ~/.botho/logs/botho.log | grep -i error
   ```

## Rollback Procedures

### When to Rollback

- Node fails to start after upgrade
- Consensus errors or chain splits detected
- Critical bug discovered in new version
- Performance degradation beyond acceptable limits

### Quick Rollback (Within 1 Hour)

If issues are detected immediately:

1. **Stop the node:**
   ```bash
   botho stop
   ```

2. **Restore previous binary:**
   ```bash
   mv /usr/local/bin/botho.backup /usr/local/bin/botho
   # Or reinstall previous version
   git checkout v<previous_version>
   cargo build --release
   ```

3. **Restore configuration (if changed):**
   ```bash
   cp ~/.botho/config.toml.backup ~/.botho/config.toml
   ```

4. **Start with previous version:**
   ```bash
   botho start
   ```

### Full Rollback (Ledger State)

If the ledger is corrupted or on wrong fork:

1. **Stop the node:**
   ```bash
   botho stop
   ```

2. **Remove corrupted ledger:**
   ```bash
   rm -rf ~/.botho/ledger
   ```

3. **Restore from backup:**
   ```bash
   cp -r ~/.botho/ledger.backup ~/.botho/ledger
   ```

4. **Reinstall previous version:**
   ```bash
   # Install previous binary
   ```

5. **Resync from backup point:**
   ```bash
   botho start
   # Node will sync from backup height
   ```

### Rollback Considerations

- **Hard fork rollbacks** require coordination with network
- **Transactions after fork point** may be lost
- **Validator penalties** may apply for extended downtime
- **Contact core team** if rollback affects consensus

## Emergency Response

### Severity Levels

| Level | Description | Response Time |
|-------|-------------|---------------|
| P1 | Network-wide consensus failure | Immediate |
| P2 | Significant functionality broken | 4 hours |
| P3 | Performance issues | 24 hours |
| P4 | Minor issues | Next release |

### P1: Network Consensus Failure

1. **Assess scope:**
   ```bash
   botho rpc status
   botho rpc peers --show-versions
   ```

2. **Check for emergency announcements:**
   - Discord: #emergency-alerts
   - Twitter: @botho_project
   - GitHub: botho-project/botho/issues

3. **Follow core team guidance:**
   - May require coordinated rollback
   - May require emergency patch
   - May require temporary network halt

### P2: Node-Level Critical Issue

1. **Capture diagnostic information:**
   ```bash
   botho rpc status > ~/botho-diagnostic.txt
   botho rpc peers >> ~/botho-diagnostic.txt
   tail -1000 ~/.botho/logs/botho.log >> ~/botho-diagnostic.txt
   ```

2. **Attempt rollback to previous version**

3. **Report issue:**
   - GitHub: Create issue with diagnostic info
   - Discord: Post in #node-operators

### Contact Information

- **Emergency Discord**: #emergency-alerts
- **GitHub Issues**: https://github.com/botho-project/botho/issues
- **Security Issues**: security@botho.foundation (PGP key available)

## Upgrade History

### v1.0.0 (Current)

**Release Date:** TBD

**Changes:**
- Initial mainnet release
- Block version 0-5 support
- Cluster tags for progressive fees (v5)

**Upgrade Notes:**
- Fresh installation required for mainnet
- No migration from testnet

---

### Template for Future Upgrades

```markdown
### v<X.Y.Z>

**Release Date:** YYYY-MM-DD

**Type:** [Hard Fork | Soft Fork | Patch]

**Activation:**
- Height: <block_height> (or)
- Timestamp: <unix_timestamp>

**Changes:**
- Change 1
- Change 2

**Breaking Changes:**
- None (or list breaking changes)

**Migration Required:** [Yes | No]

**Upgrade Notes:**
- Specific instructions for this upgrade
```

---

## Appendix: Upgrade Announcement Template

When announcing an upgrade, include:

```
BOTHO NETWORK UPGRADE ANNOUNCEMENT

Upgrade: v<old> → v<new>
Type: [Hard Fork | Soft Fork]
Activation: [Block height <N> | Timestamp <T>]

TIMELINE:
- Announcement: <date>
- Recommended upgrade: <date range>
- Activation: <date>

CHANGES:
- <summary of changes>

ACTION REQUIRED:
- [All node operators must upgrade before <date>]
- [Upgrade recommended but not required]

RESOURCES:
- Release: https://github.com/botho-project/botho/releases/tag/v<new>
- Upgrade guide: https://github.com/botho-project/botho/blob/main/UPGRADING.md
```
