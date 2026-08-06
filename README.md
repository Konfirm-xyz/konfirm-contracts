# konfirm-contracts

Four Soroban smart contracts backing [Konfirm](https://github.com/samuel2926i39-art/konfirm-backend), a non-custodial payment processor on Stellar. All four are deployed to Testnet (addresses below); none are currently called by the live checkout or cash-out flow in `konfirm-backend` — today's pilot moves value with plain Stellar classic payments, and these contracts are the on-chain layer that path is built toward.

## Contracts

| Contract | Purpose |
|---|---|
| `compliance` | Default-allow address screening with an explicit deny/allow list, pausable |
| `payment` | Attestation layer — records that a payment happened and its refund state; not an escrow |
| `treasury` | 2-of-3 multisig settlement: propose → approve → execute |
| `channel` | Escrow-backed payment channels with checkpointing, a configurable challenge period, and cooperative or unilateral close |

Every contract follows the same shape: an `initialize` entry point, a `pause`/`unpause` pair, and mutations that check the pause flag before touching state.

### `compliance`

| Function | Signature |
|---|---|
| `initialize` | `(admin: Address)` |
| `is_allowed` | `(addr: Address) -> bool` |
| `block_address` / `allow_address` / `clear_address` | `(addr: Address)` |
| `pause` / `unpause` | `()` |

Default-allow: an address is permitted unless explicitly blocked. `clear_address` restores the default rather than leaving an explicit "allowed" entry.

### `payment`

| Function | Signature |
|---|---|
| `initialize` | `(admin: Address)` |
| `record_payment` | see `lib.rs` — writes a payment record |
| `mark_refunded` | `(payment_id: u64)` — rejects a second refund on the same ID |
| `get_payment` | `(payment_id: u64) -> PaymentRecord` |
| `pause` / `unpause` | `()` |

### `treasury`

| Function | Signature |
|---|---|
| `initialize` | signer set + threshold |
| `propose_settlement` | creates a pending settlement |
| `approve_settlement` | `(signer: Address, settlement_id: u64)` — rejects non-signers and double-approval |
| `execute_settlement` | `(settlement_id: u64)` — only once threshold is met; rejects double-execution |
| `get_settlement` | `(settlement_id: u64) -> Settlement` |
| `pause` / `unpause` | `()` |

Deployed with a 2-of-3 threshold (see [Deployed addresses](#deployed-addresses-testnet)).

### `channel`

| Function | Signature |
|---|---|
| `initialize` | `(admin: Address, challenge_period_secs: u64)` — challenge period is a deployment-time parameter, not hardcoded |
| `open_channel` | opens an escrow-backed channel |
| `top_up` | `(payer: Address, channel_id: u64, amount: i128)` |
| `checkpoint` | signed state update; rejects stale nonces |
| `initiate_close` / `finalize_close` | unilateral close, respecting the challenge period; refunds the remainder once it elapses |
| `hold_channel` / `release_hold` | freezes/unfreezes checkpointing without closing |
| `get_channel_info` | `(channel_id: u64) -> Channel` |
| `pause` / `unpause` | `()` |

Signed messages are domain-separated with `DOMAIN_TAG = b"KONFIRM_CHAN_V1"` (15 bytes) prefixed onto the signing payload, so a signature from this contract can never be replayed against a different one.

The deployed instance uses `challenge_period_secs = 86400` (24 hours). Shorter/longer periods for lower/higher-value channels are a deployment decision, not a contract feature — the contract itself only enforces whatever value it was initialized with.

## Deployed addresses (Testnet)

| Contract | Address |
|---|---|
| Compliance | `CDDVLE2DZQAYFY3Z2Z74TUNNPC4ROUACSBXOB2P64IT75EZFAQXSRSXY` |
| Payment | `CCYRA6JT2L4NS5FG4B5TP52JPCGCPYSP7M6LUDUY2QA37V5UBXWJBRHV` |
| Treasury | `CDUGB6KXOEHYVEDCC673CVW33I3FXBPE5EPHXNYOVBLAAXTWZG6SFESZ` |
| Channel | `CDS2Y4CQMQWFLCG5GHVKX7UIXHYPM6IJDJZTEXSASSHGHLESGLGLNPL6` |

Deployer: `GAEMG5TVLEIQYCY3XB4EJT742DIE3FQO53RSESSYJQUZIWZOJQIZATJS`

Treasury signers (2-of-3 threshold): `GAEMG5TVLEIQYCY3XB4EJT742DIE3FQO53RSESSYJQUZIWZOJQIZATJS`, `GBRUR4UZHKPQ76S4S7X7INENL6QJ4UGNFIQ3F6VAYJQZRO3F4XBZAIND`, `GCP57AJNZIVVTPPSD4MJ2SQDMTN4QEAUP64U2OZ4HXSAU4O4A6NOWY2Z`

## Tech stack

- Rust, `soroban-sdk 27.0.5`
- `#![no_std]` contracts — no heap allocation, no std library
- 21 unit tests across the four contracts using `soroban-sdk`'s `testutils`

## Prerequisites

- Rust (stable)
- The `wasm32v1-none` target — **not** `wasm32-unknown-unknown`. `rust-toolchain.toml` currently lists `wasm32-unknown-unknown`, but `soroban-sdk 27` rejects that target on Rust 1.82+; install the correct one explicitly:

  ```bash
  rustup target add wasm32v1-none
  ```
- [Stellar CLI](https://developers.stellar.org/docs/tools/developer-tools/cli/stellar-cli) (`stellar` on `$PATH`) for building and deploying

## Build & test

```bash
cargo test
```

```bash
stellar contract build --target wasm32v1-none
```

Deploy a single contract (repeat per contract, `channel` shown, note the extra constructor arg):

```bash
stellar contract deploy \
  --wasm target/wasm32v1-none/release/channel.wasm \
  --source deployer --network testnet \
  -- --admin <admin_address> --challenge_period_secs 86400
```

## Project structure

```
contracts/
  compliance/   allow/deny-list screening
  payment/      payment attestation + refund tracking
  treasury/     multisig settlement
  channel/      escrow-backed payment channels
artifacts/      generated deployment output (gitignored — contains only public addresses, kept local to avoid churn on every redeploy)
```

## Known limitations

- Not yet called from `konfirm-backend`'s live checkout/cash-out path — the pilot uses plain classic Stellar payments today.
- `channel`'s tiered challenge-period design (shorter periods for lower-value channels, longer for higher-value ones) exists as an operational recommendation, not contract-enforced logic — `initialize` takes one value per channel deployment.
- No integration tests against a live Soroban RPC endpoint; test coverage is unit-level via `testutils` only.

## License

No license file yet — private project, all rights reserved by default.
