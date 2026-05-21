# said-register

CLI tool to register your AI agent on [SAID Protocol](https://www.saidprotocol.com) (Solana).

## Quick Start

```bash
npx said-register --metadata https://example.com/your-agent.json
```

## Installation

```bash
npm install -g said-register
```

## Usage

```bash
# Basic registration
npx said-register --metadata https://example.com/agent.json

# With custom keypair
npx said-register -m https://example.com/agent.json -k ./wallet.json

# Register AND get verified (0.01 SOL)
npx said-register -m https://example.com/agent.json --verify
```

## Options

| Option | Alias | Description |
|--------|-------|-------------|
| `--metadata <url>` | `-m` | URL to your AgentCard JSON (required) |
| `--keypair <path>` | `-k` | Path to Solana keypair (default: ~/.config/solana/id.json) |
| `--rpc <url>` | `-r` | Solana RPC URL (default: mainnet) |
| `--verify` | `-v` | Also get verified badge (costs 0.01 SOL) |
| `--help` | `-h` | Show help |

## AgentCard JSON Format

Host a JSON file at a public URL with your agent's metadata:

```json
{
  "name": "Your Agent Name",
  "description": "What your agent does",
  "twitter": "@handle",
  "website": "https://yoursite.com",
  "capabilities": ["skill1", "skill2"]
}
```

## Costs

- **Registration**: ~0.03 SOL (rent for on-chain account)
- **Verification**: 0.01 SOL (optional verified badge)

## More Info

- Website: https://www.saidprotocol.com
- Skill.md: https://www.saidprotocol.com/skill.md
- SDK: `npm install said-sdk`

## License

MIT
