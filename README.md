# konfirm-contracts

Four Soroban smart contracts backing [Konfirm](https://github.com/konfirm-HQ/konfirm-backend), a non-custodial payment processor on Stellar. All four are deployed to Testnet (addresses below); none are currently called by the live checkout or cash-out flow in `konfirm-backend` — today's pilot moves value with plain Stellar classic payments, and these contracts are the on-chain layer that path is built toward.

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
| `set_admin` | `(current_admin: Address, new_admin: Address)` — both must authorize; not pause-gated |
| `pause` / `unpause` | `()` |

Default-allow: an address is permitted unless explicitly blocked. `clear_address` restores the default rather than leaving an explicit "allowed" entry.

**Advisory, not enforced on-chain.** `compliance` is consulted off-chain by `konfirm-backend` (`isAllowedOnChain`) before it calls `payment::record_payment` or proposes a `treasury` settlement. The `payment`, `treasury`, and `channel` contracts never call `compliance`, so a blocked address is not rejected by them. Treat `compliance` as the screening source of truth for the backend, not as a hard on-chain gate. Wiring it in as a gate would mean passing the compliance contract ID into `payment` and `treasury` at initialization and checking `is_allowed` in `record_payment`, `propose_settlement`, and `execute_settlement`.

### `payment`

| Function | Signature |
|---|---|
| `initialize` | `(admin: Address)` |
| `record_payment` | see `lib.rs` — writes a payment record |
| `mark_refunded` | `(payment_id: u64)` — rejects a second refund on the same ID |
| `get_payment` | `(payment_id: u64) -> PaymentRecord` |
| `set_admin` | `(current_admin: Address, new_admin: Address)` — both must authorize; not pause-gated |
| `pause` / `unpause` | `()` |

### `treasury`

| Function | Signature |
|---|---|
| `initialize` | signer set + threshold |
| `propose_settlement` | creates a pending settlement |
| `approve_settlement` | `(signer: Address, settlement_id: u64)` — rejects non-signers and double-approval |
| `execute_settlement` | `(caller: Address, settlement_id: u64)` — caller must be a signer; only once threshold is met; rejects double-execution |
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
| `set_admin` | `(current_admin: Address, new_admin: Address)` — both must authorize; not pause-gated |
| `pause` / `unpause` | `()` |

Signed messages are domain-separated with `DOMAIN_TAG = b"KONFIRM_CHAN_V1"` (15 bytes) prefixed onto the signing payload, so a signature from this contract can never be replayed against a different one.

The deployed instance uses `challenge_period_secs = 86400` (24 hours). Shorter/longer periods for lower/higher-value channels are a deployment decision, not a contract feature — the contract itself only enforces whatever value it was initialized with.

## Deployed addresses (Testnet)

Redeployed 2026-10-22 (PRD M1) from commit `851fa1a1260c83a5d8dfd896b5f8c33932d26ab5` — the last commit confirmed to build and pass its full test suite. The tip of `main` at redeploy time (`1583b79`) did not compile; see [konfirm-contracts#237](https://github.com/Konfirm-xyz/konfirm-contracts/issues/237). Each address below was verified with `stellar contract info interface` against this exact source commit.

| Contract | Address |
|---|---|
| Compliance | `CCDMRUZ2JSP7LKGKE7MGOTLKM26IMX6OYJBD7XQHZPKFZMXKOZ32OZ7A` |
| Payment | `CDUDDS5OZKJSB7UO7MYU6MMSUQ4YN5Z3UKFV6GSLDM2HYTMCH7LKRZRH` |
| Treasury | `CDPSQANQMGKE4XRVRZHE57HBWKRJ6KKWAUJ6TBVMZYJ47GZTIOO2KLT4` |
| Channel | `CCFIIEFAAYNKDABKKI2N5Z7MZDMQY7Q5BD6BMIZDE5SNKW6JCLC63GOC` |

Deployer: `GAEMG5TVLEIQYCY3XB4EJT742DIE3FQO53RSESSYJQUZIWZOJQIZATJS`

Treasury signers (2-of-3 threshold): `GAEMG5TVLEIQYCY3XB4EJT742DIE3FQO53RSESSYJQUZIWZOJQIZATJS`, `GBRUR4UZHKPQ76S4S7X7INENL6QJ4UGNFIQ3F6VAYJQZRO3F4XBZAIND`, `GCP57AJNZIVVTPPSD4MJ2SQDMTN4QEAUP64U2OZ4HXSAU4O4A6NOWY2Z`

Treasury was initialized with the real testnet USDC SAC (`CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA`). The channel contract's token allowlist (`set_token_allowed`) was set for both testnet USDC (above) and testnet EURC (`CCUUDM434BMZMYWYDITHFXHDMIVTGGD6T2I5UKNX5BSLXLW7HVR4MCGZ`, derived from issuer `GB3Q6QDZYTHWT7E5PVS3W7FUT5GVAFC5KSZFFLPU25GO7VTC3NM2ZTVO` via `stellar contract id asset`).

## Tech stack

- Rust, `soroban-sdk 27.0.5`
- `#![no_std]` contracts — no heap allocation, no std library
- 36 unit tests across the four contracts using `soroban-sdk`'s `testutils` (at `851fa1a`; the figure below was stale before this redeploy)

## Prerequisites

- Rust (stable)
- The `wasm32v1-none` target — **not** `wasm32-unknown-unknown`. `rust-toolchain.toml` currently lists `wasm32-unknown-unknown`, but `soroban-sdk 27` rejects that target on Rust 1.82+; install the correct one explicitly:

  ```bash
  rustup target add wasm32v1-none
  ```
- [Stellar CLI](https://developers.stellar.org/docs/tools/developer-tools/cli/stellar-cli) (`stellar` on `$PATH`) for building and deploying

## Build & test

```bash
cargo test --all --features testutils
```

The `testutils` feature must be enabled. Plain `cargo test` fails to compile, because the test code imports `soroban_sdk::testutils`. CI runs the command above.

```bash
stellar contract build --target wasm32v1-none
```

Deploy, then initialize. These contracts have **no constructor**: passing `-- --admin ...` at deploy time does not set them up. The `initialize` call is separate. The `channel` example:

```bash
stellar contract deploy \
  --wasm target/wasm32v1-none/release/konfirm_channel.wasm \
  --source deployer --network testnet
# -> prints the contract id, <CONTRACT_ID>

stellar contract invoke --id <CONTRACT_ID> \
  --source deployer --network testnet \
  -- initialize --admin <admin_address> --challenge_period_secs 86400
```

Each contract's `initialize` takes its own arguments: `compliance` and `payment` take `--admin`; `treasury` takes `--admin --token --signers --threshold`; `channel` takes `--admin --challenge_period_secs`. Calling it twice fails with `AlreadyInitialized`, which is the protection.

Verified on testnet: a deploy without `initialize` leaves every admin call failing with `NotInitialized` (error 2).

## Authorization convention

Every `pub fn` in `contracts/*/src/lib.rs` must do one of two things. CI enforces this with `python3 scripts/check_pub_fn_auth.py`, and the check fails the build otherwise:

- **Authorized:** its body calls `require_auth`, `require_signer`, or `require_admin` (directly or via a helper of that name).
- **Permissionless:** its doc comment has a `/// Permissionless` line explaining why no check is needed. Examples: `channel::finalize_close`, `channel::checkpoint` (authorized by the payer's signature), and read-only getters.

If you add a `pub fn` that is neither, the check will fail. Either add the auth check or document the function as permissionless and say why.

## Project structure

```
contracts/
  compliance/   allow/deny-list screening
  payment/      payment attestation + refund tracking
  treasury/     multisig settlement
  channel/      escrow-backed payment channels
artifacts/      local deployment output (gitignored — contains only public addresses, kept local to avoid churn on every redeploy).
                NOT authoritative: it is never regenerated automatically and can silently go stale after a redeploy.
                Use the [Deployed addresses](#deployed-addresses-testnet) table above as the source of truth.
```

## Known limitations

- Not yet called from `konfirm-backend`'s live checkout/cash-out path — the pilot uses plain classic Stellar payments today.
- `channel`'s tiered challenge-period design (shorter periods for lower-value channels, longer for higher-value ones) exists as an operational recommendation, not contract-enforced logic — `initialize` takes one value per channel deployment.
- No integration tests against a live Soroban RPC endpoint; test coverage is unit-level via `testutils` only.

## License

[MIT](LICENSE)
