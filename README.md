# TrusTrove Contracts

Decentralized trade finance protocol on Stellar. SMEs tokenize unpaid invoices and receive immediate USDC funding from a liquidity pool. Built with Soroban smart contracts.

## Architecture

Four Soroban contracts in a Cargo workspace, deployed in dependency order:

```
registry → invoice → escrow → pool
```

### Contract Overview

| Contract | Purpose |
|---|---|
| **registry** | Single source of truth for verified issuers and buyers |
| **invoice** | Full invoice lifecycle — create, list, fund, ship, confirm, repay, default |
| **escrow** | USDC custody between funding and repayment, callable only by pool |
| **pool** | USDC liquidity pool with share-based LP accounting and yield distribution |

### Invoice Lifecycle

```
CREATED → LISTED → FUNDED → ACTIVE → CONFIRMED → REPAID
                                    ↘ DEFAULTED
```

## Prerequisites

- Rust 1.74+ with `wasm32-unknown-unknown` target
- Stellar CLI (`stellar`)

```bash
rustup target add wasm32-unknown-unknown
```

## Build

```bash
# Build all contracts (debug)
cargo build --workspace

# Build WASM release artifacts
cargo build --workspace --release --target wasm32-unknown-unknown

# Run all tests
cargo test --workspace
```

WASM files are output to `target/wasm32-unknown-unknown/release/*.wasm`.

## Test

```bash
# Run all workspace tests
cargo test --workspace

# Run tests for a specific contract
cargo test -p trusttrove-pool
```

## Deploy to Testnet

### 1. Create and fund deployer account

```bash
chmod +x scripts/setup-testnet.sh
./scripts/setup-testnet.sh
```

### 2. Deploy all contracts

```bash
chmod +x scripts/deploy.sh
./scripts/deploy.sh
```

The deploy script:
1. Builds all contracts to WASM
2. Deploys registry → invoice → escrow → pool in order
3. Initializes each contract with the correct cross-contract addresses
4. Outputs contract IDs for use in the frontend

### 3. Configure frontend

Copy the contract IDs from the deploy output into your `trusttrove-app` `.env.local`:

```
NEXT_PUBLIC_REGISTRY_CONTRACT_ID=...
NEXT_PUBLIC_INVOICE_CONTRACT_ID=...
NEXT_PUBLIC_ESCROW_CONTRACT_ID=...
NEXT_PUBLIC_POOL_CONTRACT_ID=...
```

## Environment Variables

See `.env.example` for all required variables:

| Variable | Description |
|---|---|
| `STELLAR_NETWORK` | Network name (`testnet` or `mainnet`) |
| `STELLAR_RPC_URL` | Soroban RPC endpoint |
| `STELLAR_NETWORK_PASSPHRASE` | Network passphrase for signing |
| `DEPLOYER_ACCOUNT` | Stellar CLI key alias for deployment |
| `USDC_ISSUER` | USDC asset issuer on Stellar |
| `USDC_ASSET_CODE` | Asset code (`USDC`) |

## Cross-Contract Dependencies

```
registry_contract (source of truth for verification)
    ↑
invoice_contract (calls registry.is_verified)
    ↑↓
pool_contract (calls invoice.get_status, mark_funded)
    ↑↓
escrow_contract (called by pool for lock/release)
```

All cross-contract addresses are stored in instance storage at `initialize()` time.

## Security

- All amounts in stroops (1 USDC = 10,000,000 stroops)
- Percentage math uses basis points (bps), never floating point
- Integer overflow protection enabled in release profile
- Every persistent `set()` is followed by `extend_ttl()` to prevent storage expiry
- Admin-only functions gated by `require_auth()` on the stored admin address
- Cross-contract calls authenticated via contract address `require_auth()`
