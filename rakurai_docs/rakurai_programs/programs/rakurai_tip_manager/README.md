# Rakurai Tip Manager Program

A Solana smart contract for managing tips sent to validators. The program maintains **eight tip accounts** to reduce write-lock contention and, on each Rakurai leader turn, drains them into the validator’s **Tips Collection Account (TCA)** in the [Reward Distribution](../reward_distribution/README.md) program.

➤ IDL: [rakurai_tip_manager.json](./idl/rakurai_tip_manager.json).

---

## 1. Deployed program ID

### Mainnet

- **Tip Manager program:** [rKtiPTD7WuCdEEQ2JXWgAmZHHL9iZLc3niCXwtS7wSH](https://solscan.io/account/rKtiPTD7WuCdEEQ2JXWgAmZHHL9iZLc3niCXwtS7wSH)
- **Tip accounts:**
  - `BjqjPHFmwr19YFmkH8CMNJFbj1wzX9k9ngr4am2nQEdq`
  - `9CNKnAqJgLA4pL6KByzhhdY4mKoQP5wcPdhJgnvvi5Ve`
  - `5wy4C2VMFhHE4i8PWKNS1K4SV275zjNwhLwfKBwajrro`
  - `AgMdA97pk2i2Ry4YQ4iVPNrRiFhcH3x3ARUCiQGt3vJG`
  - `4Qf8JFV5vmpADXNouoJriQ9KiniT5DENrz9JM2mKGH9m`
  - `AuFAFzbzE9dzMajy4RNdyJZBTskeiuJQqT2wd9xoGSRD`
  - `8aLaHz8595MAvgxKoBJEyZmDfqQp8CorezFGYnC7CPjy`
  - `H6hyJo6rpBmwHbvVuWCEHExJ2bE4rcn1hTPeiBtypus4`

### Testnet

- **Tip Manager program:** [4qRZaFzf7MvgfBTCP9grb69cCST8UmKHPtkpGAgkJosD](https://solscan.io/account/4qRZaFzf7MvgfBTCP9grb69cCST8UmKHPtkpGAgkJosD?cluster=testnet)
- **Tip accounts:**
  - `3ahyXyni1jLj8kJ13VgGEFDJzB374dgQW273nJSg8cdm`
  - `3aebD4TAn1somZfiaKRrMypUfmbDzT7XMVWRM5TFHuKW`
  - `Hm4LFyTAbrgH4eejYmNXQJ9oejQyq8frD2qeJbmkCAWR`
  - `AffPqNJ8jSrFGgfiouVfXcra1Vd6gHUjNhpoL8uW8dY5`
  - `9Z4pSxRZzE1T2e6587yzMWtvo8RHKW3R5Rb2FcprUPz`
  - `J2JdwcRrxWyCHKrgi2ipwCFXK2oRSgzPN4P7Q6Kz9XZ9`
  - `DscP7KHpAvfnboSKEQ5KEcwuFuRWn6MTjKYYTftuqY6z`
  - `Ur14r1oNyLvYeFLngGoEwYV4zwFVcui72vJqAavDXhZ`

---

## 2. How it works

A singleton `TipManagerConfigAccount` stores:

- **`validator_tip_receiver_account`** — current drain destination (the validator’s Rakurai TCA)
- **`client_commission_account` / `client_commission_bps`** — Rakurai cut used on the **next** drain (synced from the TCA that was just claimed)
- **`authority`** — config updater
- **`bumps`** — PDA bumps for the eight tip accounts

`change_tip_receiver_v2` drains using the **current** global commission (set by the previous leader), then copies commission fields from the **new** TCA for the next leader.

Eight separate tip PDAs exist so many tippers can land at once without serializing on a single write lock.

---

## 3. Account structure

### 3.1. TipManagerConfigAccount

Singleton PDA (`TIP_MANAGER_CONFIG_ACCOUNT`):

| Field | Role |
|-------|------|
| `authority` | Authorized config updater |
| `validator_tip_receiver_account` | Account receiving the validator tip share |
| `client_commission_account` | Rakurai commission destination |
| `client_commission_bps` | Commission in basis points (0–10000) |
| `bumps` | Bumps for the eight tip PDAs |

### 3.2. Rakurai tip accounts

Empty state PDAs that hold SOL. Drain moves all lamports above rent-exempt minimum.

---

## 4. Tip distribution flow

1. **Users send tips** → any of the eight tip accounts (`SystemProgram.transfer`; no tip-manager ix required)
2. **Validator drains** with `change_tip_receiver_v2`:
   - New receiver **must** be a TCA (`REVENUE_SHARE_V1`, `share_kind = Tip`)
   - Splits drained SOL: Rakurai commission account vs new TCA
   - CPIs `record_revenue_v1` on the **old** TCA (auto-credits `transferred_amount` for the Rakurai vault)
   - Writes `validator_tip_receiver_account` = new TCA; syncs `client_commission_*` from the new TCA
3. After the epoch, Reward Distribution `claim_revenue_v1` pays the validator identity. The Rakurai-named TCA skips commission at claim (already taken on drain)

Partner **custom tip accounts** are not drained here — they use a per-service TCA and [Partner CLI settlement](../../cli/partner_reward_settlement.md). See [Reward Distribution — TCA](../reward_distribution/README.md#3-tca--tips-for-landing-transactions).

---

## 5. Integration with reward distribution

| | Value |
|--|--------|
| Tip receiver PDA | `[REVENUE_SHARE_V1, TIP, rakurai, vote]` |
| Init | `initialize_revenue_share_account_v1` |
| Drain | `change_tip_receiver_v2` |
| Record CPI | `record_revenue_v1` |

SDK: `derive_rakurai_tip_collection_v1_address`.

How money is split: [Reward Distribution](../reward_distribution/README.md).

---

## 6. Account lifecycle

- **Tip accounts:** stay open; accumulate until the next drain
- **Config account:** stays open until the authority closes it
- Drain never drops a tip PDA below rent exemption
