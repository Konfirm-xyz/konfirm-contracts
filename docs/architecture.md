# Architecture

Four Soroban contracts plus a shared helper crate, one Cargo workspace. Each contract is an independent deployment with its own address and its own `initialize` call. `payment`, `treasury`, and `channel` each make a real on-chain, same-ledger cross-contract call into an admin-configured `compliance` deployment (via `set_compliance` + a generated `ComplianceClient`), rejecting with a `ComplianceBlocked` error if the configured contract reports an address blocked. This is a no-op (advisory mode) until `set_compliance` is called, so an instance that never configures it behaves as if no cross-contract call existed. The only other external calls are to a SEP-41 token contract (`treasury` and `channel`). Everything else that connects the four still happens in the off-chain caller, `konfirm-backend`, not on-chain.

## The four contracts

| Contract | Holds funds? | Who can mutate it | What it's for |
|---|---|---|---|
| [`compliance`](../contracts/compliance/src/lib.rs) | No | Admin | Address screening. Default-allow: an address is permitted unless it is explicitly blocked. |
| [`payment`](../contracts/payment/src/lib.rs) | No | Admin | Attestation. Records that a payment happened and whether it was refunded. It is not an escrow and moves no tokens. |
| [`treasury`](../contracts/treasury/src/lib.rs) | Yes | Signers (propose, approve); anyone (execute, once the threshold is met) | 2-of-3 multisig settlement. Propose, then approve, then execute. Execution transfers the settlement token from the treasury's own balance. |
| [`channel`](../contracts/channel/src/lib.rs) | Yes | Payer opens and tops up; payer or payee starts a close; admin holds and releases; anyone finalizes a close after the challenge period | Escrow-backed off-chain payment channels. The payer locks funds on open. The payee is paid by submitting signed cumulative checkpoints. A close starts a challenge period, and anyone can finalize it once that period has elapsed, refunding the unclaimed remainder to the payer. |

Each contract also has an `initialize` entry point (one-shot, rejects a second call) and a `pause` / `unpause` pair. Pausing blocks mutations. `is_allowed` (compliance) and `checkpoint` (channel) are documented exceptions to the pause rule. See `CONTRIBUTING.md` for the authorization convention.

## The shared helper crate

[`contracts/common`](../contracts/common/src/lib.rs) (`konfirm-common`) is a library crate, not a deployable contract. It centralizes the admin and pause plumbing that all four contracts share:

- `require_admin` — checks the stored admin and rejects unauthorized callers.
- `require_not_paused` — rejects mutations while the contract is paused.
- `set_paused` — flips the stored pause flag.

Error types still differ per contract. `konfirm-common` defines its own `CommonError`, and each contract maps it into its own error enum via `From<CommonError>`. That keeps each contract's public error surface stable and self-describing while sharing the implementation of the checks.

## How they relate

```
                 konfirm-backend (off-chain)
                 ├── is_allowed?  ──────────────► compliance
                 ├── record_payment / mark_refunded ─► payment ──is_allowed?──► compliance (if set_compliance configured)
                 ├── propose / approve / execute ───► treasury ─is_allowed?──► compliance (if set_compliance configured)
                 │                                 └─(token transfer)──► payee
                 └── open / top_up / checkpoint /
                     close ────────────────────────► channel  ─is_allowed?──► compliance (if set_compliance configured;
                     │                                                        open_channel/top_up only, not checkpoint/close)
                     └─(token transfer)──► payee / payer
```

- **Compliance gates payments both off-chain and on-chain.** The payment-preparation path in `konfirm-backend` calls `is_allowed` before it builds a payment. `payment`, `treasury`, and `channel` additionally call `is_allowed` on-chain themselves, once their admin has configured `set_compliance` — `record_payment`, `propose_settlement`/`execute_settlement`, and `open_channel`/`top_up` respectively all reject with `ComplianceBlocked` rather than silently succeeding for a blocked address. Before `set_compliance` is configured on a given instance, that instance's on-chain check is a no-op, so a payment that skips the off-chain check can still be recorded and settled on an unconfigured instance.
- **Payment is a record, not a rail.** `payment` stores the attestation and refund state for a payment. Value moves through classic Stellar payments today, or through `treasury` or `channel` once those are wired in.
- **Treasury holds settlement funds.** Settlements are paid out of the treasury contract's balance. Funding the treasury is an ordinary token transfer to the contract address. The contract does not check the source of funds.
- **Channel is the high-frequency path.** Two parties exchange signed cumulative claims off-chain. The chain only sees the open, any top-ups, the latest checkpoint, and the close. Checkpoints are signed with the domain tag `KONFIRM_CHAN_V1`, so a signature cannot be replayed against another contract.

## Current status

None of the four contracts is called by the live checkout or cash-out flow yet. Today's pilot moves value with plain Stellar classic payments. See the README's "Known limitations".

## Cross-contract calls require a same-Env integration test

**This already happened, and the rule below was not enforced automatically when it did (issue #254).** `payment`, `treasury`, and `channel` each now call `compliance` on-chain (see above). Issue #62 filed this section as a forward-looking tripwire tied to a *future* netting-engine change; when the compliance wiring landed instead, it slipped through without anyone adding the same-Env test this section calls for, because the tripwire was never anything but a convention written down here — no CI job, lint, or script ever checked for it. `grep konfirm- contracts/*/Cargo.toml` (issue #62's own evidence method) would *not* have caught this either: a Soroban cross-contract call needs only a runtime `Address`, not a `Cargo.toml` dependency, so the absence of a workspace dependency edge proves nothing about whether contracts call each other.

That gap is now closed two ways:
1. Same-Env integration tests exist for every cross-contract call: `contracts/payment/src/compliance_integration_test.rs`, `contracts/treasury/src/compliance_integration_test.rs`, and `contracts/channel/src/compliance_integration_test.rs` each register `ComplianceContract` alongside the calling contract in one `Env` and assert both the `ComplianceBlocked` rejection path and the cleared/unconfigured success path.
2. `scripts/check_cross_contract_tests.py` (run in CI, see `.github/workflows/ci.yml`) statically detects any `<X>Client::new(` call in a contract's non-test source and fails the build unless some test function anywhere in the repo registers both that contract and the `<X>Contract` it calls into, in the same test function. This is the actual enforcement mechanism issue #62 asked for; the paragraph below is now a historical description of the convention, not the only thing backing it.

**Rule:** when any contract gains a call into another contract's address, a same-Env, two-contract integration test must land in the same PR. The test must deploy both contracts into one `Env` and assert the cross-contract call path behaves correctly, including the failure path (for example, what happens if the called contract is paused or panics mid-call). `scripts/check_cross_contract_tests.py` now fails CI if this is skipped.

## Things to know before changing anything

- **Deploy and initialize are separate steps.** The contracts have no constructor. A deployed but uninitialized contract rejects admin calls with `NotInitialized`. See the README's deploy section.
- **The challenge period is a deployment parameter.** `channel` takes `challenge_period_secs` at `initialize` and does not hard-code it. The deployed Testnet instance uses 86400 seconds. The contract rejects `0` with `InvalidChallengePeriod`. With `0`, `finalize_close` succeeds in the same ledger as `initiate_close`, so the payee gets no window to checkpoint a higher claim.
- **Treasury threshold is fixed at initialization.** The Testnet instance is 2-of-3. Changing it means a new deployment.

## Design decisions

- **`record_payment` keeps five flat scalar parameters (declined: input struct).** `record_payment` takes its five fields as flat scalars rather than a single input struct, even though they map field-for-field to the `PaymentRecord` it builds. The only caller is `konfirm-backend`'s reconciler, which lives in a separate repository and invokes the contract through generated client bindings; those bindings, and raw Soroban CLI invocation, are both more ergonomic with flat scalar arguments than with a nested struct. A struct parameter would therefore add signature sprawl without any practical benefit at the call site, so the flat signature is intentional and should not be re-raised as a cleanup item.
