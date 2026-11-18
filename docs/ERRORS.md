# Error Codes

Every contract exposes a `#[contracterror]` enum. Soroban returns the numeric
`u32` code in the `Error(Contract, #N)` result, so callers should match on the
code rather than the variant name. This file is the canonical reference for
those codes.

When you add, remove, or rename a variant in any `#[contracterror]` enum, update
the matching table below. There is currently no CI check that enforces this —
an earlier version of this file claimed `scripts/check_error_docs.py` did, but
that script does not exist anywhere in this repo. Keeping this table accurate
is a manual review responsibility until such a check is written.

## payment (`contracts/payment`)

| Code | Variant | Meaning |
| ---- | ------- | ------- |
| 1 | `AlreadyInitialized` | `initialize` was called more than once. |
| 2 | `NotInitialized` | The contract has not been initialized yet. |
| 3 | `Unauthorized` | Caller is not the admin/owner for this operation. |
| 4 | `Paused` | The contract is paused; mutating calls are rejected. |
| 5 | `NotFound` | The referenced payment does not exist. |
| 6 | `InvalidAmount` | Amount is zero or otherwise invalid. |
| 7 | `AlreadyRefunded` | The payment has already been refunded. |
| 8 | `AlreadyVoided` | The payment has already been voided. |
| 9 | `Voided` | The payment was voided; the operation is not allowed. |
| 10 | `DuplicateTxHash` | `record_payment`'s `tx_hash` was already recorded by an earlier call. |
| 11 | `ComplianceBlocked` | `record_payment` rejected because the configured compliance contract reports the merchant or payer as blocked. |

## treasury (`contracts/treasury`)

| Code | Variant | Meaning |
| ---- | ------- | ------- |
| 1 | `AlreadyInitialized` | `initialize` was called more than once. |
| 2 | `NotInitialized` | The contract has not been initialized yet. |
| 3 | `Unauthorized` | Caller is not the admin/owner for this operation. |
| 4 | `Paused` | The contract is paused; mutating calls are rejected. |
| 5 | `NotFound` | The referenced settlement does not exist. |
| 6 | `ZeroAmount` | `propose_settlement` called with an amount `<= 0`. |
| 7 | `NotSigner` | The caller is not in the configured signer set. |
| 8 | `AlreadyApproved` | This signer has already approved the settlement. |
| 9 | `ThresholdNotMet` | `execute_settlement` called before the settlement reached its approval threshold. |
| 10 | `NotPending` | `approve_settlement`, `execute_settlement`, or `sweep_expired_settlement` called on a settlement that is not `Pending`. |
| 11 | `InvalidThreshold` | `initialize` called with a threshold of `0` or greater than the number of signers. |
| 12 | `DuplicateSigner` | `initialize` was given the same signer address twice. |
| 13 | `NotExpired` | `sweep_expired_settlement` called before the settlement outlived `MAX_PENDING_LIFETIME`. |
| 14 | `ComplianceBlocked` | `propose_settlement` or `execute_settlement` rejected because the configured compliance contract reports the merchant as blocked. |

## channel (`contracts/channel`)

| Code | Variant | Trigger |
|---|---|---|
| 1 | `AlreadyInitialized` | `initialize` called after the contract was already set up. |
| 2 | `NotInitialized` | A call that needs stored config (admin, challenge period) made before `initialize`. |
| 3 | `Unauthorized` | `top_up` by someone other than the channel's payer, or `initiate_close` by someone other than the payer or payee. |
| 4 | `Paused` | `open_channel` or `top_up` while paused. `checkpoint`, `initiate_close` and `finalize_close` are never blocked by pause. |
| 5 | `NotFound` | Any call with a channel id that does not exist. |
| 6 | `NotOpen` | `top_up` or `initiate_close` on a channel that is not `Open`; `checkpoint` on a `Closed` channel. |
| 7 | `StaleClaim` | `checkpoint` with a `nonce` not greater than the stored one, or a `cumulative_amount` not greater than what was already claimed. |
| 8 | `ExceedsDeposit` | `checkpoint` with a `cumulative_amount` greater than the channel's deposit. |
| 9 | `ChallengeNotElapsed` | `finalize_close` before `closing_at + challenge period`. |
| 10 | `NotClosing` | `finalize_close` on a channel that is not `Closing`. |
| 11 | `Held` | `checkpoint` on a channel that is `Held` by admin. |
| 12 | `ZeroDeposit` | `open_channel` or `top_up` with an amount `<= 0`. |
| 13 | `TokenNotAllowed` | `open_channel` with a token that is not on the admin's allowlist (`set_token_allowed`). |
| 14 | `TooManyOpenChannels` | `open_channel` would put the payer over `MAX_OPEN_CHANNELS_PER_PAYER` still-open channels. |
| 15 | `InvalidChallengePeriod` | `initialize` called with `challenge_period_secs == 0`. |
| 16 | `DepositOverflow` | `top_up` would overflow the channel's `i128` deposited total. |

## compliance (`contracts/compliance`)

| Code | Variant | Meaning |
| ---- | ------- | ------- |
| 1 | `AlreadyInitialized` | `initialize` was called more than once. |
| 2 | `NotInitialized` | The contract has not been initialized yet. |
| 3 | `Unauthorized` | `set_admin` called with a `current_admin` that does not match the stored admin. |
| 4 | `Paused` | The contract is paused; `block_address`, `allow_address` and `clear_address` are rejected. |
