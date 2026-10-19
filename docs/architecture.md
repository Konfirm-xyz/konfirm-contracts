# Architecture

Four Soroban contracts, one Cargo workspace. Each contract is an independent deployment with its own address and its own `initialize` call. The contracts do not call each other: there are no cross-contract invocations in this repo. The only external calls are to a SEP-41 token contract (`treasury` and `channel`). Anything that connects the four (for example, checking `compliance` before recording a `payment`) happens in the off-chain caller, `konfirm-backend`, not on-chain.

## The four contracts

| Contract | Holds funds? | Who can mutate it | What it's for |
|---|---|---|---|
| [`compliance`](../contracts/compliance/src/lib.rs) | No | Admin | Address screening. Default-allow: an address is permitted unless it is explicitly blocked. |
| [`payment`](../contracts/payment/src/lib.rs) | No | Admin | Attestation. Records that a payment happened and whether it was refunded. It is not an escrow and moves no tokens. |
| [`treasury`](../contracts/treasury/src/lib.rs) | Yes | Signers (propose, approve); anyone (execute, once the threshold is met) | 2-of-3 multisig settlement. Propose, then approve, then execute. Execution transfers the settlement token from the treasury's own balance. |
| [`channel`](../contracts/channel/src/lib.rs) | Yes | Payer opens and tops up; payer or payee starts a close; admin holds and releases; anyone finalizes a close after the challenge period | Escrow-backed off-chain payment channels. The payer locks funds on open. The payee is paid by submitting signed cumulative checkpoints. A close starts a challenge period, and anyone can finalize it once that period has elapsed, refunding the unclaimed remainder to the payer. |

Each contract also has an `initialize` entry point (one-shot, rejects a second call) and a `pause` / `unpause` pair. Pausing blocks mutations. `is_allowed` (compliance) and `checkpoint` (channel) are documented exceptions to the pause rule. See `CONTRIBUTING.md` for the authorization convention.

## How they relate

```
                 konfirm-backend (off-chain)
                 ├── is_allowed?  ──────────────► compliance
                 ├── record_payment / mark_refunded ─► payment
                 ├── propose / approve / execute ───► treasury ──(token transfer)──► payee
                 └── open / top_up / checkpoint /
                     close ────────────────────────► channel    ──(token transfer)──► payee / payer
```

- **Compliance gates payments, but only off-chain.** The payment-preparation path in `konfirm-backend` calls `is_allowed` before it builds a payment. No contract checks it. A payment that skips the check will still be recorded and settled on-chain.
- **Payment is a record, not a rail.** `payment` stores the attestation and refund state for a payment. Value moves through classic Stellar payments today, or through `treasury` or `channel` once those are wired in.
- **Treasury holds settlement funds.** Settlements are paid out of the treasury contract's balance. Funding the treasury is an ordinary token transfer to the contract address. The contract does not check the source of funds.
- **Channel is the high-frequency path.** Two parties exchange signed cumulative claims off-chain. The chain only sees the open, any top-ups, the latest checkpoint, and the close. Checkpoints are signed with the domain tag `KONFIRM_CHAN_V1`, so a signature cannot be replayed against another contract.

## Current status

None of the four contracts is called by the live checkout or cash-out flow yet. Today's pilot moves value with plain Stellar classic payments. See the README's "Known limitations".

## Things to know before changing anything

- **Deploy and initialize are separate steps.** The contracts have no constructor. A deployed but uninitialized contract rejects admin calls with `NotInitialized`. See the README's deploy section.
- **The challenge period is a deployment parameter.** `channel` takes `challenge_period_secs` at `initialize` and does not hard-code it. The deployed Testnet instance uses 86400 seconds.
- **Treasury threshold is fixed at initialization.** The Testnet instance is 2-of-3. Changing it means a new deployment.
- **`record_payment` keeps five flat parameters on purpose (#47).** Folding `merchant, payer, link_id, amount, tx_hash` into an input struct was considered and declined:
  - In `konfirm-backend` (`src/payments/payment-attestation.service.ts`), `PaymentAttestationService.attest` calls `client.record_payment({ merchant, payer, link_id, amount, tx_hash })` via `@stellar/stellar-sdk/contract` `Client.from<PaymentContract>()`, which already wraps the contract's 5 flat parameters into a single named-argument object on the TypeScript side (`src/common/stellar-sdk-contract.d.ts`). A contract-level input struct would force a redundant `{ input: { ... } }` wrapper in TypeScript.
  - Flat scalar args map directly onto `stellar contract invoke -- record_payment --merchant … --tx_hash …`. A struct arg would require hand-written JSON on the CLI.
  - Keeping the 5 flat parameters preserves the deployed Testnet contract's ABI and existing test snapshots.
