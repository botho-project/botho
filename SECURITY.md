# Security Policy

## Reporting a Vulnerability

The Botho team takes security seriously. If you discover a security vulnerability, please report it responsibly.

### How to Report

**Do NOT open a public GitHub issue for security vulnerabilities.**

Instead, please email security concerns to:

**security@botho.io**

Include:
- A description of the vulnerability
- Steps to reproduce
- Potential impact assessment
- Any suggested fixes (optional)

### What to Expect

| Timeframe | Response |
|:----------|:---------|
| 24 hours | Acknowledgment of your report |
| 72 hours | Initial assessment and severity classification |
| 7 days | Detailed response with remediation plan |
| 90 days | Public disclosure (coordinated with you) |

### Scope

The following are in scope for security reports:

- **Cryptographic vulnerabilities** in transaction privacy, key derivation, or signatures
- **Consensus attacks** that could enable double-spending or chain reorganization
- **Network attacks** that could partition or eclipse nodes
- **Wallet vulnerabilities** that could leak keys or enable theft
- **Smart contract issues** in any on-chain logic

### Out of Scope

- Denial of service attacks (please report, but lower priority)
- Social engineering attacks
- Physical attacks requiring device access
- Vulnerabilities in dependencies (report upstream, but notify us)

### Safe Harbor

We consider security research conducted in good faith to be authorized. We will not pursue legal action against researchers who:

- Make a good faith effort to avoid privacy violations and data destruction
- Do not exploit vulnerabilities beyond proof-of-concept
- Report vulnerabilities promptly and allow reasonable time for fixes
- Do not publicly disclose before coordinated disclosure

### Recognition

We maintain a [HALL_OF_FAME.md](./HALL_OF_FAME.md) to recognize security researchers who help improve Botho. With your permission, we'll add your name after the vulnerability is resolved.

### Bug Bounty

We are evaluating a formal bug bounty program. In the meantime, significant vulnerability reports may be eligible for discretionary rewards based on severity and impact.

## Supported Versions

| Version | Supported |
|:--------|:----------|
| main branch | Yes |
| Latest release | Yes |
| Previous releases | Security fixes only |

## Security Practices

### For Users

- Store your 24-word recovery phrase offline and secure
- Verify checksums when downloading releases
- Run your own node when possible
- Keep your node software updated

### For Node Operators

- Use dedicated machines for validators
- Enable firewalls and restrict RPC access
- Monitor logs for unusual activity
- Subscribe to security announcements

## Contact

- Security issues: security@botho.io
- General questions: [GitHub Discussions](https://github.com/botho-project/botho/discussions)
